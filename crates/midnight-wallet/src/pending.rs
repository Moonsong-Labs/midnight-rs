//! In-flight spend reservations.
//!
//! A [`PendingReservations`] entry records UTXOs picked by a locally-built
//! transaction whose on-chain spends have not yet been observed via indexer
//! events. Build paths consult these at
//! [`crate::Wallet::build_context_inner`] time so they don't re-select the
//! same inputs before the chain confirms (or expires) the previous tx.
//!
//! Entries are cleared in two ways:
//! - **Confirmation** — event replay (initial sync and every resync)
//!   collects the dust nullifiers of `DustSpendProcessed` events and the
//!   `(intent_hash, output_index)` keys of spent unshielded UTXOs, then
//!   calls [`PendingReservations::clear_confirmed`] at its commit point:
//!   an unshielded reservation is removed when its exact key was spent, a
//!   dust batch when any of its spend nullifiers was observed.
//! - **TTL eviction** — if `reserved_at + global_ttl < current_chain_time`,
//!   the entry can no longer produce a valid transaction (its TTL window
//!   has elapsed) and is dropped. This is the backstop for transactions
//!   that never confirm.
//!
//! Pending state is persisted to its own file (`pending.json`) so that a
//! process restart between submit and confirmation does not lose track of
//! reservations. Confirmed state files (`metadata.json`, `zswap-N.bin`,
//! `dust_wallet-N.bin`) never carry pending entries.

use midnight_helpers::{
    DefaultDB, DustLocalState, DustNullifier, DustSpend, ProofPreimageMarker, Sp, Timestamp,
    WalletSeed,
};
use midnight_serialize::{tagged_deserialize, tagged_serialize};
use serde::{Deserialize, Serialize};

use crate::WalletError;
use crate::transfer::{DustSpendBatch, SpentUtxoKey};

/// One pending batch of dust spends from a single `speculative_spend` call.
///
/// The new helpers `mark_spent(spends, updated_state)` API requires the
/// `(spends, updated_state)` pair from one `speculative_spend` to be passed
/// together, so we preserve that grouping here. Applying the batch to a
/// `DustWallet` clone at build time re-inserts the nullifiers into
/// `spent_utxos` and rolls `dust_local_state` forward.
#[derive(Clone)]
pub(crate) struct PendingDustBatch {
    /// The funding wallet seed this batch came from.
    pub seed: WalletSeed,
    /// The spends as built. Tagged-serializable so they can be persisted.
    pub spends: Vec<DustSpend<ProofPreimageMarker, DefaultDB>>,
    /// `DustLocalState` after these spends were applied to the wallet's
    /// state at reservation time. Replayed via `mark_spent` at build time.
    pub updated_state: Sp<DustLocalState<DefaultDB>, DefaultDB>,
    /// `chain_tblock` at reservation time, for TTL eviction.
    pub reserved_at: Timestamp,
}

/// One pending unshielded UTXO reservation.
#[derive(Clone)]
pub(crate) struct PendingUnshieldedSpend {
    pub key: SpentUtxoKey,
    pub reserved_at: Timestamp,
}

/// In-memory + on-disk record of spends that recent builds have reserved
/// but whose on-chain effects have not yet been observed.
#[derive(Default, Clone)]
pub(crate) struct PendingReservations {
    dust: Vec<PendingDustBatch>,
    unshielded: Vec<PendingUnshieldedSpend>,
}

impl PendingReservations {
    /// Append new reservations from a freshly-built transaction.
    pub(crate) fn reserve(
        &mut self,
        dust_batches: Vec<DustSpendBatch>,
        unshielded: Vec<SpentUtxoKey>,
        reserved_at: Timestamp,
    ) {
        self.dust
            .extend(dust_batches.into_iter().map(|b| PendingDustBatch {
                seed: b.seed,
                spends: b.spends,
                updated_state: b.updated_state,
                reserved_at,
            }));
        self.unshielded.extend(
            unshielded
                .into_iter()
                .map(|key| PendingUnshieldedSpend { key, reserved_at }),
        );
    }

    /// View the pending dust batches; the caller (e.g.
    /// `build_context_inner`) feeds each batch into
    /// `DustWallet::mark_spent` in chronological order so the resulting
    /// `dust_local_state` reflects all reservations.
    pub(crate) fn dust_batches(&self) -> impl Iterator<Item = &PendingDustBatch> {
        self.dust.iter()
    }

    /// View the pending unshielded UTXO keys; the caller uses these to
    /// filter `unshielded_utxos` when populating the `LedgerContext`.
    pub(crate) fn unshielded_keys(&self) -> impl Iterator<Item = &SpentUtxoKey> {
        self.unshielded.iter().map(|p| &p.key)
    }

    /// True when the wallet has no in-flight reservations.
    pub(crate) fn is_empty(&self) -> bool {
        self.dust.is_empty() && self.unshielded.is_empty()
    }

    /// Drop reservations whose spends were observed confirmed on-chain.
    ///
    /// Called at the sync/resync commit points with what event replay saw:
    /// `spent_unshielded` holds the `(intent_hash, output_index)` keys of
    /// spent unshielded UTXOs, `dust_nullifiers` the nullifiers of
    /// `DustSpendProcessed` events. An unshielded reservation is removed
    /// when its exact key was spent. A dust batch is removed when ANY of
    /// its spends' nullifiers was observed, and this must stay ANY, not
    /// ALL: either the reserved tx landed (atomic, so every other spend in
    /// the batch landed with it) or a conflicting tx consumed that input,
    /// in which case the reserved tx can never apply and the whole batch
    /// is dead either way.
    pub(crate) fn clear_confirmed(
        &mut self,
        spent_unshielded: &[SpentUtxoKey],
        dust_nullifiers: &[DustNullifier],
    ) {
        if !spent_unshielded.is_empty() {
            self.unshielded
                .retain(|p| !spent_unshielded.contains(&p.key));
        }
        if !dust_nullifiers.is_empty() {
            self.dust.retain(|b| {
                !b.spends
                    .iter()
                    .any(|s| dust_nullifiers.contains(&s.old_nullifier))
            });
        }
    }

    /// Evict entries whose TTL window has elapsed.
    ///
    /// A reservation with `reserved_at + global_ttl < now` can no longer
    /// produce a valid transaction, so it is safe to drop locally and
    /// re-select the inputs on a subsequent build.
    pub(crate) fn evict_expired(&mut self, now: Timestamp, global_ttl: midnight_helpers::Duration) {
        self.dust.retain(|p| p.reserved_at + global_ttl >= now);
        self.unshielded
            .retain(|p| p.reserved_at + global_ttl >= now);
    }
}

// ---------------------------------------------------------------------------
// Persistence
// ---------------------------------------------------------------------------

/// On-disk representation of [`PendingReservations`]. `DustSpend` and
/// `Sp<DustLocalState<D>, D>` are both Tagged + Serializable, so we
/// hex-encode their `tagged_serialize` bytes to round-trip through JSON
/// without dragging the tagged-codec into the schema.
#[derive(Serialize, Deserialize, Default)]
pub(crate) struct StoredPending {
    #[serde(default)]
    pub dust: Vec<StoredPendingDustBatch>,
    #[serde(default)]
    pub unshielded: Vec<StoredPendingUnshielded>,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct StoredPendingDustBatch {
    /// Hex-encoded `WalletSeed::as_bytes()` (32 bytes).
    pub seed_hex: String,
    /// Tagged-serialized `Vec<DustSpend<ProofPreimageMarker, DefaultDB>>`, hex.
    pub spends_hex: String,
    /// Tagged-serialized `Sp<DustLocalState<DefaultDB>, DefaultDB>`, hex.
    pub updated_state_hex: String,
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
        let mut dust = Vec::with_capacity(self.dust.len());
        for p in &self.dust {
            let mut spends_buf = Vec::new();
            tagged_serialize(&p.spends, &mut spends_buf)
                .map_err(|e| WalletError::Storage(format!("serialize pending dust spends: {e}")))?;
            let mut state_buf = Vec::new();
            tagged_serialize(&p.updated_state, &mut state_buf)
                .map_err(|e| WalletError::Storage(format!("serialize pending dust state: {e}")))?;
            dust.push(StoredPendingDustBatch {
                seed_hex: hex::encode(p.seed.as_bytes()),
                spends_hex: hex::encode(&spends_buf),
                updated_state_hex: hex::encode(&state_buf),
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
            let seed = WalletSeed::try_from_hex_str(&s.seed_hex)
                .map_err(|e| WalletError::Storage(format!("parse pending seed hex: {e}")))?;
            let spends_bytes = hex::decode(&s.spends_hex)
                .map_err(|e| WalletError::Storage(format!("decode pending dust spends: {e}")))?;
            let spends: Vec<DustSpend<ProofPreimageMarker, DefaultDB>> =
                tagged_deserialize(&spends_bytes[..]).map_err(|e| {
                    WalletError::Storage(format!("deserialize pending dust spends: {e}"))
                })?;
            let state_bytes = hex::decode(&s.updated_state_hex)
                .map_err(|e| WalletError::Storage(format!("decode pending dust state: {e}")))?;
            let updated_state: Sp<DustLocalState<DefaultDB>, DefaultDB> =
                tagged_deserialize(&state_bytes[..]).map_err(|e| {
                    WalletError::Storage(format!("deserialize pending dust state: {e}"))
                })?;
            dust.push(PendingDustBatch {
                seed,
                spends,
                updated_state,
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
    use midnight_helpers::mn_ledger::dust::DustCommitment;
    use midnight_helpers::{Duration, Fr, INITIAL_PARAMETERS, KeyLocation, ProofPreimage};

    use crate::transfer::DustSpendBatch;

    fn ukey(intent_hash: &str, output_index: u32) -> SpentUtxoKey {
        SpentUtxoKey {
            intent_hash: intent_hash.to_string(),
            output_index,
        }
    }

    fn nullifier(n: u64) -> DustNullifier {
        DustNullifier(Fr::from(n))
    }

    /// A structurally-valid `DustSpend` whose identity is `nullifier(n)`.
    /// The proof is a placeholder preimage — `clear_confirmed` only looks at
    /// `old_nullifier`, and persistence round-trips the whole struct.
    fn dust_spend(n: u64) -> DustSpend<ProofPreimageMarker, DefaultDB> {
        DustSpend {
            v_fee: 1,
            old_nullifier: nullifier(n),
            new_commitment: DustCommitment(Fr::from(n + 1)),
            proof: ProofPreimage {
                inputs: Vec::new(),
                private_transcript: Vec::new(),
                public_transcript_inputs: Vec::new(),
                public_transcript_outputs: Vec::new(),
                binding_input: Fr::from(0u64),
                communications_commitment: None,
                key_location: KeyLocation(std::borrow::Cow::Borrowed("test")),
            },
        }
    }

    fn dust_batch(nullifiers: &[u64]) -> DustSpendBatch {
        DustSpendBatch {
            seed: WalletSeed::try_from_hex_str(&"00".repeat(32)).unwrap(),
            spends: nullifiers.iter().map(|&n| dust_spend(n)).collect(),
            updated_state: Sp::new(DustLocalState::new(INITIAL_PARAMETERS.dust)),
        }
    }

    #[test]
    fn default_is_empty() {
        let p = PendingReservations::default();
        assert!(p.is_empty());
        assert_eq!(p.dust_batches().count(), 0);
        assert_eq!(p.unshielded_keys().count(), 0);
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

    #[test]
    fn clear_confirmed_removes_matching_reservations_before_ttl() {
        let mut p = PendingReservations::default();
        p.reserve(
            vec![dust_batch(&[7])],
            vec![ukey("abcd", 0), ukey("abcd", 1)],
            Timestamp::from_secs(100),
        );

        // Confirm one unshielded key and the dust spend: the matched
        // unshielded reservation and the dust batch go, the other
        // unshielded reservation stays.
        p.clear_confirmed(&[ukey("abcd", 0)], &[nullifier(7)]);
        assert_eq!(p.dust_batches().count(), 0);
        let remaining: Vec<_> = p.unshielded_keys().cloned().collect();
        assert_eq!(remaining, vec![ukey("abcd", 1)]);
    }

    #[test]
    fn clear_confirmed_ignores_unrelated_events() {
        let mut p = PendingReservations::default();
        p.reserve(
            vec![dust_batch(&[7])],
            vec![ukey("abcd", 0)],
            Timestamp::from_secs(100),
        );

        // Same intent, different index; different intent, same index; and a
        // foreign dust nullifier — none of them may clear anything.
        p.clear_confirmed(&[ukey("abcd", 9), ukey("ffff", 0)], &[nullifier(99)]);
        assert_eq!(p.unshielded_keys().count(), 1);
        assert_eq!(p.dust_batches().count(), 1);
    }

    #[test]
    fn clear_confirmed_drops_dust_batch_on_any_observed_nullifier() {
        let mut p = PendingReservations::default();
        p.reserve(
            vec![dust_batch(&[7, 8]), dust_batch(&[9])],
            Vec::new(),
            Timestamp::from_secs(100),
        );

        // One observed nullifier from the first batch drops the whole batch
        // (transactions apply atomically); the second batch is untouched.
        p.clear_confirmed(&[], &[nullifier(8)]);
        let batches: Vec<_> = p.dust_batches().collect();
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].spends[0].old_nullifier, nullifier(9));
    }

    #[test]
    fn clear_confirmed_after_storage_round_trip() {
        // Simulates a process restart between reserve and confirmation:
        // reserve → save to disk → load → replay confirmed spends.
        let dir = tempfile::TempDir::new().unwrap();
        let seed = WalletSeed::try_from_hex_str(&"11".repeat(32)).unwrap();

        let mut p = PendingReservations::default();
        p.reserve(
            vec![dust_batch(&[7])],
            vec![ukey("abcd", 0)],
            Timestamp::from_secs(100),
        );
        crate::storage::save_pending(dir.path(), "undeployed", &seed, &p).unwrap();

        let mut loaded = crate::storage::load_pending(dir.path(), "undeployed", &seed)
            .unwrap()
            .expect("pending.json should exist after save");
        assert_eq!(loaded.unshielded_keys().count(), 1);
        assert_eq!(loaded.dust_batches().count(), 1);

        loaded.clear_confirmed(&[ukey("abcd", 0)], &[nullifier(7)]);
        assert!(loaded.is_empty());
    }
}
