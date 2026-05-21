//! In-flight spend reservations.
//!
//! A [`PendingReservations`] entry records a UTXO that has been picked by a
//! locally-built transaction but whose on-chain spend has not yet been
//! observed via indexer events. Build paths consult these at
//! [`crate::Wallet::build_context_inner`] time so they don't re-select the
//! same input before the chain confirms (or expires) the previous tx.
//!
//! Entries are cleared in two ways:
//! - **Confirmation** — when [`crate::Wallet::apply_dust_event`] or
//!   [`crate::Wallet::apply_unshielded_event`] receives an event whose
//!   nullifier / utxo-key matches a pending entry, the entry is removed.
//! - **TTL eviction** — if `reserved_at + global_ttl < current_chain_time`,
//!   the entry can no longer produce a valid transaction (its TTL window
//!   has elapsed) and is dropped.
//!
//! Pending state is persisted to its own file (`pending.json`) so that a
//! process restart between submit and confirmation does not lose track of
//! reservations. Confirmed state files (`metadata.json`, `zswap-N.bin`,
//! `dust_wallet-N.bin`) never carry pending entries.

use midnight_node_ledger_helpers::midnight_serialize::tagged_deserialize;
use midnight_node_ledger_helpers::{DefaultDB, DustSpend, ProofPreimageMarker, Timestamp};
use serde::{Deserialize, Serialize};

use crate::WalletError;
use crate::transfer::SpentUtxoKey;

/// One pending dust spend. The full [`DustSpend`] is retained so that, on
/// reload from disk, we can re-apply [`DustWallet::mark_spent`] without
/// reconstructing the proof — the helpers' `mark_spent` only reads
/// `old_nullifier`, but the type's API requires a full `DustSpend`.
#[derive(Clone)]
pub struct PendingDustSpend {
    /// The spend as built. Tagged-serializable so it can be persisted.
    pub spend: DustSpend<ProofPreimageMarker, DefaultDB>,
    /// Approximate wall-clock time the reservation was created. Used for
    /// TTL eviction. Stored as `chain_tblock` at build time (not local
    /// wall clock) so the comparison matches the chain's TTL semantics.
    pub reserved_at: Timestamp,
}

/// One pending unshielded UTXO reservation.
#[derive(Clone)]
pub struct PendingUnshieldedSpend {
    pub key: SpentUtxoKey,
    pub reserved_at: Timestamp,
}

/// In-memory + on-disk record of spends that recent builds have reserved
/// but whose on-chain effects have not yet been observed.
#[derive(Default, Clone)]
pub struct PendingReservations {
    dust: Vec<PendingDustSpend>,
    unshielded: Vec<PendingUnshieldedSpend>,
}

impl PendingReservations {
    /// Append new reservations from a freshly-built transaction.
    pub fn reserve(
        &mut self,
        dust: Vec<DustSpend<ProofPreimageMarker, DefaultDB>>,
        unshielded: Vec<SpentUtxoKey>,
        reserved_at: Timestamp,
    ) {
        self.dust.extend(
            dust.into_iter()
                .map(|spend| PendingDustSpend { spend, reserved_at }),
        );
        self.unshielded.extend(
            unshielded
                .into_iter()
                .map(|key| PendingUnshieldedSpend { key, reserved_at }),
        );
    }

    /// View the pending dust spends; the caller (e.g. `build_context_inner`)
    /// feeds these into `DustWallet::mark_spent`.
    pub fn dust_spends(&self) -> impl Iterator<Item = &DustSpend<ProofPreimageMarker, DefaultDB>> {
        self.dust.iter().map(|p| &p.spend)
    }

    /// View the pending unshielded UTXO keys; the caller uses these to
    /// filter `unshielded_utxos` when populating the `LedgerContext`.
    pub fn unshielded_keys(&self) -> impl Iterator<Item = &SpentUtxoKey> {
        self.unshielded.iter().map(|p| &p.key)
    }

    /// True when the wallet has no in-flight reservations.
    pub fn is_empty(&self) -> bool {
        self.dust.is_empty() && self.unshielded.is_empty()
    }

    /// Drop dust reservations whose `old_nullifier` matches `nullifier`.
    ///
    /// Called when a `DustSpendProcessed` event arrives for one of our
    /// nullifiers: the spend is now confirmed on-chain and no longer needs
    /// to be tracked locally.
    pub fn confirm_dust_nullifier(
        &mut self,
        nullifier: &midnight_node_ledger_helpers::DustNullifier,
    ) {
        self.dust.retain(|p| &p.spend.old_nullifier != nullifier);
    }

    /// Drop unshielded reservations whose key matches the given (intent_hash,
    /// output_index) pair. Called when a confirmed `UnshieldedTransaction`
    /// reports the input as spent.
    pub fn confirm_unshielded(&mut self, intent_hash: &str, output_index: u32) {
        self.unshielded
            .retain(|p| !(p.key.intent_hash == intent_hash && p.key.output_index == output_index));
    }

    /// Evict entries whose TTL window has elapsed.
    ///
    /// A dust spend with `reserved_at + global_ttl < now` can no longer
    /// produce a valid transaction, so it is safe to drop locally and
    /// re-select the input on a subsequent build.
    pub fn evict_expired(
        &mut self,
        now: Timestamp,
        global_ttl: midnight_node_ledger_helpers::Duration,
    ) {
        self.dust.retain(|p| p.reserved_at + global_ttl >= now);
        self.unshielded
            .retain(|p| p.reserved_at + global_ttl >= now);
    }
}

// ---------------------------------------------------------------------------
// Persistence
// ---------------------------------------------------------------------------

/// On-disk representation of [`PendingReservations`]. The `DustSpend` field
/// is hex-encoded `tagged_serialize` bytes so we can round-trip the proof
/// without bringing the helpers' tagged-codec into JSON.
#[derive(Serialize, Deserialize, Default)]
pub(crate) struct StoredPending {
    #[serde(default)]
    pub dust: Vec<StoredPendingDustSpend>,
    #[serde(default)]
    pub unshielded: Vec<StoredPendingUnshielded>,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct StoredPendingDustSpend {
    /// Tagged-serialized `DustSpend<ProofPreimageMarker, DefaultDB>`, hex.
    pub spend_hex: String,
    /// `Timestamp::to_secs()` value.
    pub reserved_at_secs: u64,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct StoredPendingUnshielded {
    pub intent_hash: String,
    pub output_index: u32,
    pub reserved_at_secs: u64,
}

impl PendingReservations {
    pub(crate) fn to_stored(&self) -> Result<StoredPending, WalletError> {
        use midnight_node_ledger_helpers::midnight_serialize::tagged_serialize;

        let mut dust = Vec::with_capacity(self.dust.len());
        for p in &self.dust {
            let mut buf = Vec::new();
            tagged_serialize(&p.spend, &mut buf)
                .map_err(|e| WalletError::Storage(format!("serialize pending dust: {e}")))?;
            dust.push(StoredPendingDustSpend {
                spend_hex: hex::encode(&buf),
                reserved_at_secs: p.reserved_at.to_secs(),
            });
        }

        let unshielded = self
            .unshielded
            .iter()
            .map(|p| StoredPendingUnshielded {
                intent_hash: p.key.intent_hash.clone(),
                output_index: p.key.output_index,
                reserved_at_secs: p.reserved_at.to_secs(),
            })
            .collect();

        Ok(StoredPending { dust, unshielded })
    }

    pub(crate) fn from_stored(stored: StoredPending) -> Result<Self, WalletError> {
        let mut dust = Vec::with_capacity(stored.dust.len());
        for s in stored.dust {
            let bytes = hex::decode(&s.spend_hex)
                .map_err(|e| WalletError::Storage(format!("decode pending dust hex: {e}")))?;
            let spend: DustSpend<ProofPreimageMarker, DefaultDB> =
                tagged_deserialize(&bytes[..])
                    .map_err(|e| WalletError::Storage(format!("deserialize pending dust: {e}")))?;
            dust.push(PendingDustSpend {
                spend,
                reserved_at: Timestamp::from_secs(s.reserved_at_secs),
            });
        }

        let unshielded = stored
            .unshielded
            .into_iter()
            .map(|s| PendingUnshieldedSpend {
                key: SpentUtxoKey {
                    intent_hash: s.intent_hash,
                    output_index: s.output_index,
                },
                reserved_at: Timestamp::from_secs(s.reserved_at_secs),
            })
            .collect();

        Ok(Self { dust, unshielded })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use midnight_node_ledger_helpers::Duration;

    fn ukey(intent_hash: &str, output_index: u32) -> SpentUtxoKey {
        SpentUtxoKey {
            intent_hash: intent_hash.to_string(),
            output_index,
        }
    }

    #[test]
    fn default_is_empty() {
        let p = PendingReservations::default();
        assert!(p.is_empty());
        assert_eq!(p.dust_spends().count(), 0);
        assert_eq!(p.unshielded_keys().count(), 0);
    }

    #[test]
    fn unshielded_confirm_clears_matching_key() {
        let mut p = PendingReservations::default();
        let ts = Timestamp::from_secs(100);
        p.reserve(
            Vec::new(),
            vec![ukey("abcd", 0), ukey("abcd", 1), ukey("ef01", 0)],
            ts,
        );
        assert_eq!(p.unshielded_keys().count(), 3);

        p.confirm_unshielded("abcd", 0);
        assert_eq!(p.unshielded_keys().count(), 2);

        // confirming a non-matching key leaves the set unchanged
        p.confirm_unshielded("00", 99);
        assert_eq!(p.unshielded_keys().count(), 2);

        p.confirm_unshielded("abcd", 1);
        p.confirm_unshielded("ef01", 0);
        assert!(p.is_empty());
    }

    #[test]
    fn evict_expired_drops_entries_past_ttl() {
        let mut p = PendingReservations::default();
        // Reserved at t=100 with a 30-second TTL window.
        p.reserve(Vec::new(), vec![ukey("abcd", 0)], Timestamp::from_secs(100));
        let ttl = Duration::from_secs(30);

        // now = 100 + 20: still inside the window.
        p.evict_expired(Timestamp::from_secs(120), ttl);
        assert_eq!(p.unshielded_keys().count(), 1);

        // now = 100 + 30: at the boundary — we keep entries with
        // `reserved_at + ttl >= now`, so 130 is still inside.
        p.evict_expired(Timestamp::from_secs(130), ttl);
        assert_eq!(p.unshielded_keys().count(), 1);

        // now = 100 + 31: past the boundary.
        p.evict_expired(Timestamp::from_secs(131), ttl);
        assert!(p.is_empty());
    }
}
