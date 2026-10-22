//! In-flight spend reservations.
//!
//! A [`PendingReservations`] entry records UTXOs picked by a locally-built
//! transaction whose on-chain spends have not yet been observed via indexer
//! events. Build paths consult these at
//! [`crate::Wallet::build_context_inner`] time so they don't re-select the
//! same inputs before the chain confirms (or expires) the previous tx.
//!
//! Entries are cleared three ways:
//! - **Confirmation** — event replay (initial sync and every resync)
//!   collects the dust nullifiers of `DustSpendProcessed` events and the
//!   `(intent_hash, output_index)` keys of spent unshielded UTXOs, then
//!   calls [`PendingReservations::clear_confirmed`] at its commit point:
//!   an unshielded reservation is removed when its exact key was spent, a
//!   dust batch when any of its spend nullifiers was observed. Shielded
//!   coins are deliberately absent: a confirmed spend drops the coin from
//!   `zswap_state`, so its reservation is already inert.
//! - **Release** — [`PendingReservations::release`], for a build known to be
//!   dead.
//! - **TTL eviction** — if `reserved_at + global_ttl < current_chain_time`,
//!   the entry can no longer produce a valid transaction (its TTL window
//!   has elapsed) and is dropped. This is the backstop for transactions
//!   that neither confirm nor get released.
//!
//! Pending state is persisted to its own file (`pending.json`) so that a
//! process restart between submit and confirmation does not lose track of
//! reservations. Confirmed state files (`metadata.json`, `zswap-N.bin`,
//! `dust_wallet-N.bin`) never carry pending entries.

use std::path::Path;

use super::helpers;
use helpers::{DefaultDB, DustNullifier, DustSpend, Nullifier, ProofPreimageMarker, Timestamp};
use tracing::warn;

use super::types::DustSpendBatch;
use crate::storage::{
    StoredPending, StoredPendingDustBatch, StoredPendingShielded, StoredPendingUnshielded,
    tagged_from_hex, tagged_to_hex,
};
use crate::{ReservedInput, SpentUtxoKey, WalletError};

/// One pending batch of dust spends from a single `speculative_spend` call.
///
/// A later build applies the `spends` of each batch to a `DustWallet` clone
/// with the clone's current state, so its coin selection skips the reserved
/// nullifiers.
#[derive(Clone)]
pub(crate) struct PendingDustBatch {
    /// The spends as built. Tagged-serializable so they can be persisted.
    pub spends: Vec<DustSpend<ProofPreimageMarker, DefaultDB>>,
    /// `chain_tblock` at reservation time, for TTL eviction.
    pub reserved_at: Timestamp,
}

impl PendingDustBatch {
    /// Whether a spend of this batch has one of `nullifiers`.
    ///
    /// ANY, not ALL: one match stands for the whole batch, because a
    /// transaction applies atomically. The other spends landed with the
    /// matched one, or none did.
    fn spends_any_of(&self, nullifiers: &[DustNullifier]) -> bool {
        nullifiers.iter().any(|n| self.spends(n))
    }

    fn spends(&self, nullifier: &DustNullifier) -> bool {
        self.spends.iter().any(|s| s.old_nullifier == *nullifier)
    }
}

/// One pending unshielded UTXO reservation.
#[derive(Clone)]
pub(crate) struct PendingUnshieldedSpend {
    pub key: SpentUtxoKey,
    pub reserved_at: Timestamp,
}

/// One pending shielded (Zswap) coin reservation, pinned by its nullifier.
#[derive(Clone)]
pub(crate) struct PendingShieldedSpend {
    pub nullifier: Nullifier,
    pub reserved_at: Timestamp,
}

/// In-memory + on-disk record of spends that recent builds have reserved
/// but whose on-chain effects have not yet been observed.
#[derive(Default, Clone)]
pub(crate) struct PendingReservations {
    dust: Vec<PendingDustBatch>,
    unshielded: Vec<PendingUnshieldedSpend>,
    shielded: Vec<PendingShieldedSpend>,
}

impl PendingReservations {
    /// Append new reservations from a freshly-built transaction.
    pub(crate) fn reserve(
        &mut self,
        dust_batches: Vec<DustSpendBatch>,
        unshielded: Vec<SpentUtxoKey>,
        shielded: Vec<Nullifier>,
        reserved_at: Timestamp,
    ) {
        self.dust
            .extend(dust_batches.into_iter().map(|b| PendingDustBatch {
                spends: b.spends,
                reserved_at,
            }));
        self.unshielded.extend(
            unshielded
                .into_iter()
                .map(|key| PendingUnshieldedSpend { key, reserved_at }),
        );
        self.shielded
            .extend(shielded.into_iter().map(|nullifier| PendingShieldedSpend {
                nullifier,
                reserved_at,
            }));
    }

    /// View the pending dust batches. A build marks their spends as spent on
    /// its `DustWallet` clone, so its coin selection skips them.
    pub(crate) fn dust_batches(&self) -> impl Iterator<Item = &PendingDustBatch> {
        self.dust.iter()
    }

    /// View the pending unshielded UTXO keys; the caller uses these to
    /// filter `unshielded_utxos` when populating the `LedgerContext`.
    pub(crate) fn unshielded_keys(&self) -> impl Iterator<Item = &SpentUtxoKey> {
        self.unshielded.iter().map(|p| &p.key)
    }

    /// View the pending shielded coin nullifiers; the caller removes these
    /// coins from the build context's Zswap state and from
    /// `spendable_shielded_coins` so a coin an unconfirmed build already spent
    /// is not re-selected.
    pub(crate) fn shielded_nullifiers(&self) -> impl Iterator<Item = &Nullifier> {
        self.shielded.iter().map(|p| &p.nullifier)
    }

    /// True when the wallet has no in-flight reservations.
    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.dust.is_empty() && self.unshielded.is_empty() && self.shielded.is_empty()
    }

    /// Drop reservations whose spends were observed confirmed on-chain.
    ///
    /// Called at the sync/resync commit points with what event replay saw:
    /// `spent_unshielded` holds the `(intent_hash, output_index)` keys of
    /// spent unshielded UTXOs, `dust_nullifiers` the nullifiers of
    /// `DustSpendProcessed` events. An unshielded reservation is removed
    /// when its exact key was spent. A dust batch is removed when ANY of
    /// its spends' nullifiers was observed ([`PendingDustBatch::spends_any_of`]).
    /// This also holds when a conflicting tx consumed that input: the
    /// reserved tx can then never apply, so the whole batch is dead.
    ///
    /// Returns true when it removed a reservation.
    pub(crate) fn clear_confirmed(
        &mut self,
        spent_unshielded: &[SpentUtxoKey],
        dust_nullifiers: &[DustNullifier],
    ) -> bool {
        let before = (self.unshielded.len(), self.dust.len());
        if !spent_unshielded.is_empty() {
            self.unshielded
                .retain(|p| !spent_unshielded.contains(&p.key));
        }
        if !dust_nullifiers.is_empty() {
            self.dust.retain(|b| !b.spends_any_of(dust_nullifiers));
        }
        (self.unshielded.len(), self.dust.len()) != before
    }

    /// Drop the reservations a specific build took, because that build will
    /// never reach the chain.
    ///
    /// Reserving on build is what stops a later build re-selecting the same
    /// inputs, but a transaction that is rejected at submit, or simply
    /// abandoned, would otherwise hold them until its TTL elapses. Callers that
    /// know the transaction is dead hand back what the build reported spending.
    ///
    /// Matching mirrors [`Self::clear_confirmed`]: an unshielded entry goes by
    /// its exact key, a shielded entry by its nullifier, and a dust batch when
    /// it spends a named nullifier.
    ///
    /// Each named input also carries its own `reserved_at`, and an entry goes
    /// only when its stamp matches, so a build hands back only what it
    /// reserved. Two builds cannot hold one input at the same `reserved_at`,
    /// because the first one's reservation takes that input out of the second
    /// one's selection. Without the timestamp a late release drops whatever
    /// entry now holds the input, and a third build then selects an input the
    /// second one is still spending.
    ///
    /// Release each reservation once. `reserved_at` is a block timestamp, so a
    /// second release of the same inputs can drop an entry another build made
    /// in that same block after the first release freed them.
    pub(crate) fn release(
        &mut self,
        dust: &[ReservedInput<DustNullifier>],
        unshielded: &[ReservedInput<SpentUtxoKey>],
        shielded: &[ReservedInput<Nullifier>],
    ) {
        self.dust.retain(|b| {
            !dust
                .iter()
                .any(|d| d.reserved_at == b.reserved_at && b.spends(&d.input))
        });
        self.unshielded.retain(|p| {
            !unshielded
                .iter()
                .any(|u| u.reserved_at == p.reserved_at && u.input == p.key)
        });
        self.shielded.retain(|p| {
            !shielded
                .iter()
                .any(|s| s.reserved_at == p.reserved_at && s.input == p.nullifier)
        });
    }

    /// Evict the entries outside [`within_ttl`], so a later build can select
    /// their inputs again.
    pub(crate) fn evict_expired(&mut self, now: Timestamp, global_ttl: helpers::Duration) {
        self.dust
            .retain(|p| within_ttl(p.reserved_at, now, global_ttl));
        self.unshielded
            .retain(|p| within_ttl(p.reserved_at, now, global_ttl));
        self.shielded
            .retain(|p| within_ttl(p.reserved_at, now, global_ttl));
    }
}

/// Whether a reservation made at `reserved_at` can still produce a valid
/// transaction at `now`.
///
/// Past `reserved_at + global_ttl`, the transaction it was made for is dead, so
/// its inputs are free again.
pub(crate) fn within_ttl(
    reserved_at: Timestamp,
    now: Timestamp,
    global_ttl: helpers::Duration,
) -> bool {
    reserved_at + global_ttl >= now
}

// ---------------------------------------------------------------------------
// Persistence
// ---------------------------------------------------------------------------

impl PendingReservations {
    pub(crate) fn to_stored(&self) -> Result<StoredPending, WalletError> {
        let mut dust = Vec::with_capacity(self.dust.len());
        for p in &self.dust {
            dust.push(StoredPendingDustBatch {
                spends_hex: tagged_to_hex(&p.spends, "pending dust spends")?,
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

        let mut shielded = Vec::with_capacity(self.shielded.len());
        for p in &self.shielded {
            shielded.push(StoredPendingShielded {
                nullifier_hex: tagged_to_hex(&p.nullifier, "pending shielded nullifier")?,
                reserved_at_secs: p.reserved_at.to_secs(),
            });
        }

        Ok(StoredPending {
            ledger_version: super::LEDGER,
            dust,
            unshielded,
            shielded,
        })
    }

    /// Persist these reservations to the wallet's `pending.json`.
    pub(crate) fn save(
        &self,
        base: &Path,
        network: &str,
        wallet_id: &str,
    ) -> Result<(), WalletError> {
        crate::storage::save_pending(base, network, wallet_id, &self.to_stored()?)
    }

    /// The reservations the wallet's `pending.json` holds, if it has one.
    pub(crate) fn load(
        base: &Path,
        network: &str,
        wallet_id: &str,
    ) -> Result<Option<Self>, WalletError> {
        crate::storage::load_pending(base, network, wallet_id)?
            .map(Self::from_stored)
            .transpose()
    }

    /// Decode what a `pending.json` holds. Reservations another generation
    /// made are dropped: a transaction built for it cannot land on a chain
    /// that runs this one.
    pub(crate) fn from_stored(stored: StoredPending) -> Result<Self, WalletError> {
        if stored.ledger_version != super::LEDGER {
            warn!(
                found = %stored.ledger_version,
                "dropping pending reservations of another ledger generation"
            );
            return Ok(Self::default());
        }
        let mut dust = Vec::with_capacity(stored.dust.len());
        for s in stored.dust {
            dust.push(PendingDustBatch {
                spends: tagged_from_hex(&s.spends_hex, "pending dust spends")?,
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

        let mut shielded = Vec::with_capacity(stored.shielded.len());
        for s in stored.shielded {
            shielded.push(PendingShieldedSpend {
                nullifier: tagged_from_hex(&s.nullifier_hex, "pending shielded nullifier")?,
                reserved_at: Timestamp::from_secs(s.reserved_at_secs),
            });
        }

        Ok(Self {
            dust,
            unshielded,
            shielded,
        })
    }
}

#[cfg(test)]
pub(super) mod test_support {
    use super::*;
    use crate::Seed;
    use helpers::mn_ledger::dust::DustCommitment;
    use helpers::{DustLocalState, Fr, INITIAL_PARAMETERS, KeyLocation, ProofPreimage, Sp};

    pub(crate) fn ukey(intent_hash: &str, output_index: u32) -> SpentUtxoKey {
        SpentUtxoKey {
            intent_hash: intent_hash.to_string(),
            output_index,
        }
    }

    pub(crate) fn nullifier(n: u64) -> DustNullifier {
        DustNullifier(Fr::from(n))
    }

    /// A structurally valid `DustSpend` whose identity is `nullifier(n)`.
    /// The proof is a placeholder preimage. The pending paths read only
    /// `old_nullifier`, and persistence round-trips the whole struct.
    pub(crate) fn dust_spend(n: u64) -> DustSpend<ProofPreimageMarker, DefaultDB> {
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

    /// A batch of [`dust_spend`]s. Its seed is a fixed placeholder, because a
    /// reservation drops the seed.
    pub(crate) fn dust_batch(nullifiers: &[u64]) -> DustSpendBatch {
        DustSpendBatch {
            seed: Seed::from_bytes(&[0x00; 32]).unwrap(),
            spends: nullifiers.iter().map(|&n| dust_spend(n)).collect(),
            updated_state: Sp::new(DustLocalState::new(INITIAL_PARAMETERS.dust)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::{dust_batch, dust_spend, nullifier, ukey};
    use super::*;
    use crate::Seed;
    use helpers::{Duration, DustLocalState, INITIAL_PARAMETERS, Sp};

    /// A transaction built for one generation cannot land on a chain that
    /// runs another, so its reservations are dead. Loading them would hide
    /// their inputs until their TTL, and would decode another generation's
    /// Dust spends with this one's types.
    #[test]
    fn reservations_of_another_generation_are_dropped_at_load() {
        let mut p = PendingReservations::default();
        p.reserve(
            Vec::new(),
            vec![ukey("abcd", 0)],
            Vec::new(),
            Timestamp::from_secs(100),
        );
        for version in [
            midnight_types::LedgerVersion::V8,
            midnight_types::LedgerVersion::V9,
        ] {
            let mut stored = p.to_stored().unwrap();
            stored.ledger_version = version;
            let loaded = PendingReservations::from_stored(stored).unwrap();
            assert_eq!(
                loaded.unshielded_keys().count() == 1,
                version == super::super::LEDGER,
                "{version} reservations loaded into a {} wallet",
                super::super::LEDGER
            );
        }
    }

    /// An earlier build wrote each Dust batch with an `updated_state_hex`. A
    /// `deny_unknown_fields` on the stored structs refuses that file, and the
    /// resume of each upgraded wallet with a Dust reservation in flight fails.
    #[test]
    fn a_pending_file_that_an_earlier_build_wrote_still_loads() {
        let batch = dust_batch(&[1]);
        let body = serde_json::json!({
            "ledger_version": super::super::LEDGER,
            "dust": [{
                "spends_hex": tagged_to_hex(&batch.spends, "spends").unwrap(),
                "updated_state_hex": tagged_to_hex(&batch.updated_state, "state").unwrap(),
                "reserved_at_secs": 100,
            }],
        });
        let stored = serde_json::from_value(body).expect("the file parses");
        let loaded = PendingReservations::from_stored(stored).unwrap();
        let batches: Vec<_> = loaded.dust_batches().collect();
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].spends[0].old_nullifier, nullifier(1));
    }

    #[test]
    fn evict_expired_drops_entries_past_ttl() {
        let mut p = PendingReservations::default();
        // Reserved at t=100 with a 30-second TTL window.
        p.reserve(
            vec![dust_batch(&[7])],
            vec![ukey("abcd", 0)],
            vec![shielded_nf(1)],
            Timestamp::from_secs(100),
        );
        let ttl = Duration::from_secs(30);
        let counts = |r: &PendingReservations| {
            (
                r.dust_batches().count(),
                r.unshielded_keys().count(),
                r.shielded_nullifiers().count(),
            )
        };

        // now = 100 + 20: still inside the window.
        p.evict_expired(Timestamp::from_secs(120), ttl);
        assert_eq!(counts(&p), (1, 1, 1));

        // now = 100 + 30: at the boundary — we keep entries with
        // `reserved_at + ttl >= now`, so 130 is still inside.
        p.evict_expired(Timestamp::from_secs(130), ttl);
        assert_eq!(counts(&p), (1, 1, 1));

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
            Vec::new(),
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
            Vec::new(),
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

    /// The snapshot directory is named by public address, so no file in it
    /// needs the seed, and `pending.json` is a file in it. A seed written there
    /// would sit in the clear beside the other state files.
    #[test]
    fn pending_json_contains_no_seed_material() {
        let dir = tempfile::TempDir::new().unwrap();

        // A seed whose hex is distinctive enough to find in the raw file.
        let seed = Seed::from_bytes(&[0xab; 32]).unwrap();
        let mut p = PendingReservations::default();
        p.reserve(
            vec![DustSpendBatch {
                seed: seed.clone(),
                spends: vec![dust_spend(1)],
                updated_state: Sp::new(DustLocalState::new(INITIAL_PARAMETERS.dust)),
            }],
            vec![ukey("abcd", 0)],
            vec![shielded_nf(1)],
            Timestamp::from_secs(100),
        );
        p.save(dir.path(), "undeployed", "testwallet").unwrap();

        let raw = std::fs::read_to_string(
            dir.path()
                .join("undeployed")
                .join("testwallet")
                .join("pending.json"),
        )
        .expect("pending.json should exist after save");

        assert!(
            !raw.contains(&"ab".repeat(32)),
            "pending.json must not contain the wallet seed: {raw}"
        );
        assert!(
            !raw.to_lowercase().contains("seed"),
            "pending.json must carry no seed field at all: {raw}"
        );

        // The batch still round-trips; the seed comes from the owning wallet.
        let loaded = PendingReservations::load(dir.path(), "undeployed", "testwallet")
            .unwrap()
            .expect("pending.json should load");
        let batches: Vec<_> = loaded.dust_batches().collect();
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].spends[0].old_nullifier, nullifier(1));
    }

    #[test]
    fn clear_confirmed_after_storage_round_trip() {
        // Simulates a process restart between reserve and confirmation:
        // reserve → save to disk → load → replay confirmed spends.
        let dir = tempfile::TempDir::new().unwrap();

        let mut p = PendingReservations::default();
        p.reserve(
            vec![dust_batch(&[7])],
            vec![ukey("abcd", 0)],
            Vec::new(),
            Timestamp::from_secs(100),
        );
        p.save(dir.path(), "undeployed", "testwallet").unwrap();

        let mut loaded = PendingReservations::load(dir.path(), "undeployed", "testwallet")
            .unwrap()
            .expect("pending.json should exist after save");
        assert_eq!(loaded.unshielded_keys().count(), 1);
        assert_eq!(loaded.dust_batches().count(), 1);

        loaded.clear_confirmed(&[ukey("abcd", 0)], &[nullifier(7)]);
        assert!(loaded.is_empty());
    }

    fn shielded_nf(n: u8) -> Nullifier {
        Nullifier(helpers::HashOutput([n; 32]))
    }

    #[test]
    fn shielded_reservation_survives_storage_round_trip() {
        let dir = tempfile::TempDir::new().unwrap();

        let mut p = PendingReservations::default();
        p.reserve(
            Vec::new(),
            Vec::new(),
            vec![shielded_nf(3), shielded_nf(4)],
            Timestamp::from_secs(100),
        );
        p.save(dir.path(), "undeployed", "testwallet").unwrap();

        let loaded = PendingReservations::load(dir.path(), "undeployed", "testwallet")
            .unwrap()
            .expect("pending.json should exist after save");
        let got: Vec<_> = loaded.shielded_nullifiers().cloned().collect();
        assert_eq!(got, vec![shielded_nf(3), shielded_nf(4)]);
    }

    /// A build that cannot land must give its inputs back at once. Waiting for
    /// TTL eviction would keep them unspendable for the whole window, which is
    /// the cost of reserving on build.
    #[test]
    fn release_returns_only_the_named_reservations() {
        let mut p = PendingReservations::default();
        p.reserve(
            vec![dust_batch(&[1, 2]), dust_batch(&[3])],
            vec![ukey("aaaa", 0), ukey("bbbb", 1)],
            vec![shielded_nf(1), shielded_nf(2)],
            Timestamp::from_secs(100),
        );

        p.release(
            &[at(nullifier(1), 100)],
            &[at(ukey("aaaa", 0), 100)],
            &[at(shielded_nf(1), 100)],
        );

        // A dust batch is atomic, so matching one of its spends drops the whole
        // batch and leaves the unrelated one alone.
        assert_eq!(
            p.dust_batches()
                .map(|b| b.spends[0].old_nullifier)
                .collect::<Vec<_>>(),
            vec![nullifier(3)]
        );
        assert_eq!(
            p.unshielded_keys().cloned().collect::<Vec<_>>(),
            vec![ukey("bbbb", 1)]
        );
        assert_eq!(
            p.shielded_nullifiers().cloned().collect::<Vec<_>>(),
            vec![shielded_nf(2)]
        );
    }

    /// A late release must not take back an input a later build now holds.
    ///
    /// Build A reserves an input and releases it late. Build B reserves the
    /// same input in between, which it can only do once A's entry is gone.
    /// Matching on the input alone would drop B's entry, and a third build
    /// would then select an input B is still spending.
    #[test]
    fn release_leaves_a_later_build_s_reservation_alone() {
        let mut p = PendingReservations::default();
        // B's reservation, made after A's entry left the set.
        p.reserve(
            vec![dust_batch(&[1])],
            vec![ukey("aaaa", 0)],
            vec![shielded_nf(1)],
            Timestamp::from_secs(200),
        );

        // A releases late, naming the same inputs but its own older stamp.
        p.release(
            &[at(nullifier(1), 100)],
            &[at(ukey("aaaa", 0), 100)],
            &[at(shielded_nf(1), 100)],
        );

        assert_eq!(p.dust_batches().count(), 1, "A's release dropped B's dust");
        assert_eq!(
            p.unshielded_keys().count(),
            1,
            "A's release dropped B's unshielded entry"
        );
        assert_eq!(
            p.shielded_nullifiers().count(),
            1,
            "A's release dropped B's shielded entry"
        );
    }

    /// One release can name the entries of two reservations with different
    /// stamps, as a deploy with a shielded offer does: its pinned coins carry
    /// the stamp of its own build context, and its fee Dust that of a fresh
    /// one. Each entry goes by its own stamp. A release that applied one stamp
    /// to every entry would keep one of the two reservations.
    #[test]
    fn one_release_hands_back_entries_of_two_stamps() {
        let mut p = PendingReservations::default();
        // Build A, the pinned coins.
        p.reserve(
            Vec::new(),
            vec![ukey("aaaa", 0)],
            vec![shielded_nf(1)],
            Timestamp::from_secs(100),
        );
        // Build B, the fee Dust.
        p.reserve(
            vec![dust_batch(&[1])],
            Vec::new(),
            Vec::new(),
            Timestamp::from_secs(200),
        );
        // Build D, which the release does not name.
        p.reserve(
            Vec::new(),
            vec![ukey("bbbb", 1)],
            Vec::new(),
            Timestamp::from_secs(200),
        );

        p.release(
            &[at(nullifier(1), 200)],
            &[at(ukey("aaaa", 0), 100)],
            &[at(shielded_nf(1), 100)],
        );

        assert_eq!(p.dust_batches().count(), 0, "B's Dust stayed reserved");
        assert_eq!(
            p.shielded_nullifiers().count(),
            0,
            "A's shielded coin stayed reserved"
        );
        assert_eq!(
            p.unshielded_keys().cloned().collect::<Vec<_>>(),
            vec![ukey("bbbb", 1)],
            "the release must drop A's UTXO and keep D's"
        );
    }

    fn at<T>(input: T, reserved_at_secs: u64) -> ReservedInput<T> {
        ReservedInput {
            input,
            reserved_at: Timestamp::from_secs(reserved_at_secs),
        }
    }
}
