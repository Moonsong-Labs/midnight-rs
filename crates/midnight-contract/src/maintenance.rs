//! Contract maintenance / governance.
//!
//! A contract's on-chain `maintenance_authority` is a k-of-n committee that
//! controls verifier-key rotation and authority replacement. This SDK does not
//! hold any signing key: you set the committee (verifying keys) at deploy, and
//! every maintenance op is signed externally — you get the bytes to sign, the
//! committee members sign them with their own keys, and you submit the
//! transaction with the collected signatures. See
//! `docs/contract-maintenance-governance.md` for the protocol model.

use std::future::{Future, IntoFuture};
use std::pin::Pin;

use midnight_base_crypto::signatures::{Signature, SigningKey, VerifyingKey};
use midnight_onchain_runtime::state::{ContractMaintenanceVerifyingKey, EntryPointBuf};
use midnight_provider::{Builds, PendingTx, Provider};
use midnight_typed_state::{ContractMaintenanceAuthority, ContractState, InMemoryDB};
use midnight_types::{LedgerVersion, WalletError};

use crate::contract::{AsMidnightProvider, Contract};
use crate::error::ContractError;

// ---------------------------------------------------------------------------
// Deploy-side: stamp a committee into the contract state.
// ---------------------------------------------------------------------------

/// Set the contract's maintenance authority to `committee` with the given
/// `threshold`, `counter = 0`.
///
/// This is what makes a freshly-built deploy state governable. The default
/// authority produced by codegen has an empty committee, which can never
/// authorize a maintenance update.
pub(crate) fn set_maintenance_authority(
    mut state: ContractState<InMemoryDB>,
    committee: Vec<VerifyingKey>,
    threshold: u32,
) -> ContractState<InMemoryDB> {
    state.maintenance_authority = ContractMaintenanceAuthority {
        committee: committee
            .into_iter()
            .map(ContractMaintenanceVerifyingKey::Schnorr)
            .collect(),
        threshold,
        counter: 0,
    };
    state
}

/// Validate a committee + threshold before it is committed on-chain: the
/// committee must be non-empty and `1 <= threshold <= committee.len()`.
///
/// Guards against permanently un-maintainable contracts (`threshold` above the
/// committee size, or an empty committee) and — critically — `threshold == 0`,
/// which the ledger accepts with **zero** signatures (anyone could then govern
/// the contract).
pub(crate) fn validate_committee(
    committee: &[VerifyingKey],
    threshold: u32,
) -> Result<(), ContractError> {
    if committee.is_empty() {
        return Err(ContractError::Maintenance(
            "maintenance committee must have at least one member".into(),
        ));
    }
    if threshold == 0 {
        return Err(ContractError::Maintenance(
            "maintenance threshold must be at least 1 (threshold 0 would let anyone govern the \
             contract)"
                .into(),
        ));
    }
    if threshold as usize > committee.len() {
        return Err(ContractError::Maintenance(format!(
            "maintenance threshold {threshold} exceeds committee size {}; it could never be met",
            committee.len()
        )));
    }
    // Reject duplicate members: a committee like [vk, vk] with threshold 2 would
    // be satisfiable by a single key signing at two indices, collapsing k-of-n.
    for (i, vk) in committee.iter().enumerate() {
        if committee[..i].contains(vk) {
            return Err(ContractError::Maintenance(
                "maintenance committee contains a duplicate verifying key".into(),
            ));
        }
    }
    Ok(())
}

/// Validate that the signatures over `data` will satisfy `committee` /
/// `threshold` the way the ledger does: each signature's committee index is
/// in range and **distinct**, each verifies over `data`, and the count of
/// distinct valid signatures meets the threshold. Turns on-chain rejections
/// (`NotNormalized` / `KeyNotInCommittee` / `InvalidCommitteeSignature` /
/// `ThresholdMissed`) into early, specific errors.
fn validate_signatures(
    data: &[u8],
    signatures: &[(u32, Signature)],
    committee: &[VerifyingKey],
    threshold: u32,
) -> Result<(), ContractError> {
    let mut seen = std::collections::HashSet::new();
    for (idx, sig) in signatures {
        let vk = committee.get(*idx as usize).ok_or_else(|| {
            ContractError::Maintenance(format!(
                "signature index {idx} is outside the committee (size {})",
                committee.len()
            ))
        })?;
        if !seen.insert(*idx) {
            return Err(ContractError::Maintenance(format!(
                "duplicate signature for committee index {idx}"
            )));
        }
        if !vk.verify(data, sig) {
            return Err(ContractError::Maintenance(format!(
                "signature for committee index {idx} does not verify"
            )));
        }
    }
    if (seen.len() as u32) < threshold {
        return Err(ContractError::Maintenance(format!(
            "not enough valid signatures: have {}, authority threshold is {threshold}",
            seen.len()
        )));
    }
    Ok(())
}

/// The committee's Schnorr keys. The SDK's maintenance API signs with Schnorr
/// keys only, so a committee with another kind of member is refused.
fn schnorr_committee(
    authority: &ContractMaintenanceAuthority,
) -> Result<Vec<VerifyingKey>, ContractError> {
    authority
        .committee
        .iter()
        .map(|member| match member {
            ContractMaintenanceVerifyingKey::Schnorr(key) => Ok(key.clone()),
            _ => Err(ContractError::Maintenance(
                "the maintenance committee has a member that is not a Schnorr key, which this \
                 SDK cannot sign for"
                    .into(),
            )),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Preconditions, checked against the fetched on-chain state.
// ---------------------------------------------------------------------------

/// Entry-point key for a circuit name, as stored in `ContractState.operations`.
fn entry_point(circuit: &str) -> EntryPointBuf {
    circuit.as_bytes().into()
}

/// Validate a sequence of verifier-key operations against the on-chain state,
/// simulating each in order so a batch is checked the way the ledger applies it.
///
/// `ops` is `(circuit, is_insert)` in submission order. Insert requires the
/// circuit absent (it never replaces); remove requires it present. Because the
/// effects are simulated, `remove("x")` then `insert("x", ..)` in one batch is
/// valid even though the lone insert would not be.
fn validate_vk_sequence(
    state: &ContractState<InMemoryDB>,
    ops: &[(&str, bool)],
) -> Result<(), ContractError> {
    let mut presence: std::collections::HashMap<&str, bool> = std::collections::HashMap::new();
    for &(circuit, is_insert) in ops {
        let present = match presence.get(circuit) {
            Some(p) => *p,
            None => state.operations.contains_key(&entry_point(circuit)),
        };
        if is_insert && present {
            return Err(ContractError::Maintenance(format!(
                "circuit '{circuit}' already has a verifier key; remove it before inserting"
            )));
        }
        if !is_insert && !present {
            return Err(ContractError::Maintenance(format!(
                "circuit '{circuit}' has no verifier key to remove"
            )));
        }
        // Simulate the effect for later steps in the batch.
        presence.insert(circuit, is_insert);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Public API: Contract::at(..).maintenance()
// ---------------------------------------------------------------------------

/// Builder for one maintenance transaction. Obtained via
/// [`Contract::maintenance`](crate::Contract::maintenance).
///
/// Chain one or more operations — they are applied **in order, atomically** in a
/// single signed update — then call [`Self::prepare`]. Common batch: rotate a
/// verifier key with `remove_verifier_key(c)` then `insert_verifier_key(c, vk)`.
pub struct ContractMaintenance<'a, P> {
    contract: &'a Contract<P>,
    specs: Vec<OpSpec>,
}

impl<'a, P> ContractMaintenance<'a, P> {
    pub(crate) fn new(contract: &'a Contract<P>) -> Self {
        Self {
            contract,
            specs: Vec::new(),
        }
    }

    /// Add an insert-verifier-key step. Errors (at `prepare`) if the circuit is
    /// already defined at that point in the batch. `verifier_key` is the raw
    /// bytes of a compiled `*.verifier` artifact.
    pub fn insert_verifier_key(
        mut self,
        circuit: impl Into<String>,
        verifier_key: impl Into<Vec<u8>>,
    ) -> Self {
        self.specs.push(OpSpec::Insert {
            circuit: circuit.into(),
            verifier_key: verifier_key.into(),
        });
        self
    }

    /// Add a remove-verifier-key step. Errors (at `prepare`) if the circuit is
    /// not defined at that point in the batch.
    pub fn remove_verifier_key(mut self, circuit: impl Into<String>) -> Self {
        self.specs.push(OpSpec::Remove {
            circuit: circuit.into(),
        });
        self
    }

    /// Add a replace-authority step installing `committee`/`threshold`.
    pub fn replace_authority(mut self, committee: Vec<VerifyingKey>, threshold: u32) -> Self {
        self.specs.push(OpSpec::Replace {
            committee,
            threshold,
        });
        self
    }

    /// Fetch the current authority state, validate the batch (simulating each
    /// step in order), and build the unsigned maintenance update. The returned
    /// [`PreparedMaintenance`] exposes the bytes each committee member signs.
    pub async fn prepare(self) -> Result<PreparedMaintenance<'a, P>, ContractError>
    where
        P: Provider + AsMidnightProvider,
    {
        // Boxed; see the frame-size note on `MidnightProvider::resync_wallet`.
        Box::pin(self.prepare_inner()).await
    }

    async fn prepare_inner(self) -> Result<PreparedMaintenance<'a, P>, ContractError>
    where
        P: Provider + AsMidnightProvider,
    {
        if self.specs.is_empty() {
            return Err(ContractError::Maintenance(
                "no maintenance operations to perform".into(),
            ));
        }

        let provider = self.contract.provider().as_midnight_provider();
        let address_hex = self.contract.address();
        let address = crate::address::parse_address(address_hex)?;

        // Read the current authority at latest (the signed counter must match
        // what the chain will check at submission — not any pinned block).
        let (state_bytes, state) = crate::state::node_state(provider, address_hex, None).await?;
        let counter = state.maintenance_authority.counter;
        let threshold = state.maintenance_authority.threshold;
        let committee = schnorr_committee(&state.maintenance_authority)?;

        // Validate the verifier-key steps as a sequence (replace steps don't
        // touch the operations map).
        let vk_ops: Vec<(&str, bool)> = self
            .specs
            .iter()
            .filter_map(|s| match s {
                OpSpec::Insert { circuit, .. } => Some((circuit.as_str(), true)),
                OpSpec::Remove { circuit } => Some((circuit.as_str(), false)),
                OpSpec::Replace { .. } => None,
            })
            .collect();
        validate_vk_sequence(&state, &vk_ops)?;

        // At most one ReplaceAuthority per update (a later one would silently
        // overwrite an earlier one on apply), and each new committee must be
        // satisfiable.
        let mut replaces = 0usize;
        for spec in &self.specs {
            if let OpSpec::Replace {
                committee,
                threshold,
            } = spec
            {
                replaces += 1;
                validate_committee(committee, *threshold)?;
            }
        }
        if replaces > 1 {
            return Err(ContractError::Maintenance(
                "a maintenance update may contain at most one replace_authority".into(),
            ));
        }

        // The update is built for the generation the wallet's state is in,
        // because the bytes the committee signs differ per generation.
        let update = match provider.builds().await? {
            Builds::Ledger8(_) => Update::Ledger8(crate::ledger_8::maintenance::prepare_update(
                &state,
                &state_bytes,
                address,
                &self.specs,
                counter,
            )?),
            Builds::Ledger9(_) => Update::Ledger9(crate::ledger_9::maintenance::prepare_update(
                &state,
                &state_bytes,
                address,
                &self.specs,
                counter,
            )?),
        };
        Ok(PreparedMaintenance {
            contract: self.contract,
            update,
            signatures: Vec::new(),
            committee,
            required_threshold: threshold,
        })
    }
}

/// One step of a maintenance update, before it takes a generation's types.
pub(crate) enum OpSpec {
    Insert {
        circuit: String,
        verifier_key: Vec<u8>,
    },
    Remove {
        circuit: String,
    },
    Replace {
        committee: Vec<VerifyingKey>,
        threshold: u32,
    },
}

/// An unsigned (or partially-signed) maintenance update. Collect the committee
/// signatures with [`Self::sign`] / [`Self::add_signature`], then `.await`
/// (build + submit → [`PendingTx`]) or [`Self::build`] (proven bytes only).
///
/// Unlike [`Contract::call_with`] and [`crate::DeployBuilder`], `.await` here
/// returns the [`PendingTx`] **without** waiting for finality, so the caller chooses
/// the wait semantics (as with transfers). That means the caller owns the
/// verdict check: drive [`PendingTx::wait_finalized`] and inspect
/// `TxInBlock::verdict` — `Success` means the authority update applied, while
/// `PartialSuccess` / `Failure` mean it did not. (`call_with` and deploy make
/// that check internally and surface [`ContractError::TransactionFailed`].)
pub struct PreparedMaintenance<'a, P> {
    contract: &'a Contract<P>,
    update: Update,
    /// The signatures collected so far, as `(committee index, signature)`.
    signatures: Vec<(u32, Signature)>,
    /// The on-chain committee at prepare time — used to verify attached
    /// signatures before submission.
    committee: Vec<VerifyingKey>,
    required_threshold: u32,
}

/// An unsigned maintenance update, in the generation it was prepared for.
enum Update {
    Ledger8(midnight_helpers::ledger_8::MaintenanceUpdate<midnight_helpers::DefaultDB>),
    Ledger9(midnight_helpers::ledger_9::MaintenanceUpdate<midnight_helpers::DefaultDB>),
}

impl Update {
    fn ledger_version(&self) -> LedgerVersion {
        match self {
            Self::Ledger8(_) => LedgerVersion::V8,
            Self::Ledger9(_) => LedgerVersion::V9,
        }
    }
}

impl<'a, P> PreparedMaintenance<'a, P> {
    /// The ledger generation the update was prepared for. Its signatures are
    /// valid on that generation only.
    pub fn ledger_version(&self) -> LedgerVersion {
        self.update.ledger_version()
    }

    /// The exact bytes each committee member signs (with
    /// [`SigningKey::sign`](midnight_base_crypto::signatures::SigningKey::sign)).
    /// Distribute these to the members; collect their signatures via
    /// [`Self::add_signature`].
    pub fn data_to_sign(&self) -> Vec<u8> {
        match &self.update {
            Update::Ledger8(update) => update.data_to_sign(),
            Update::Ledger9(update) => update.data_to_sign(),
        }
    }

    /// Attach a signature produced (anywhere) over [`Self::data_to_sign`], at the
    /// signer's position in the on-chain committee.
    pub fn add_signature(mut self, committee_index: u32, signature: Signature) -> Self {
        self.signatures.push((committee_index, signature));
        self
    }

    /// Convenience for the local case: sign [`Self::data_to_sign`] with `key` and
    /// attach it at `committee_index`.
    pub fn sign(self, committee_index: u32, key: &SigningKey) -> Self {
        let signature = key.sign(&mut rand::thread_rng(), &self.data_to_sign());
        self.add_signature(committee_index, signature)
    }

    /// Check the attached signatures against the committee/threshold captured at
    /// prepare time — distinct in-range indices, each verifying, count >=
    /// threshold — so an under-signed or malformed set fails here rather than
    /// after paying to build and submit.
    fn check_signatures(&self) -> Result<(), ContractError> {
        validate_signatures(
            &self.data_to_sign(),
            &self.signatures,
            &self.committee,
            self.required_threshold,
        )
    }

    /// Build, prove, and balance the transaction without submitting it. Errors if
    /// fewer than the authority threshold of signatures have been attached.
    ///
    /// An update prepared before the chain's hard fork cannot be built after
    /// it: its signatures cover the earlier generation's bytes. Prepare it
    /// again and collect new signatures.
    pub async fn build(self) -> Result<Vec<u8>, ContractError>
    where
        P: Provider + AsMidnightProvider,
    {
        // Boxed; see the frame-size note on `MidnightProvider::resync_wallet`.
        Box::pin(self.build_inner()).await
    }

    async fn build_inner(self) -> Result<Vec<u8>, ContractError>
    where
        P: Provider + AsMidnightProvider,
    {
        self.check_signatures()?;
        let provider = self.contract.provider().as_midnight_provider();
        let builds = provider.builds().await?;
        match (builds, self.update) {
            (Builds::Ledger8(builds), Update::Ledger8(update)) => {
                Box::pin(crate::ledger_8::maintenance::maintenance_funded(
                    &builds,
                    update,
                    &self.signatures,
                ))
                .await
            }
            (Builds::Ledger9(builds), Update::Ledger9(update)) => {
                Box::pin(crate::ledger_9::maintenance::maintenance_funded(
                    &builds,
                    update,
                    &self.signatures,
                ))
                .await
            }
            (builds, update) => Err(ContractError::from(midnight_provider::ProviderError::from(
                WalletError::LedgerMismatch {
                    expected: builds.ledger_version(),
                    found: update.ledger_version(),
                },
            ))),
        }
    }
}

impl<'a, P> IntoFuture for PreparedMaintenance<'a, P>
where
    P: Provider + AsMidnightProvider + Send + Sync + 'a,
{
    type Output = Result<PendingTx, ContractError>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send + 'a>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let provider = self.contract.provider().as_midnight_provider();
            let bytes = self.build_inner().await?;
            Ok(provider.submit(&bytes).await?)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use midnight_typed_state::{StateValue, StorageHashMap};

    fn empty_state() -> ContractState<InMemoryDB> {
        ContractState::new(
            StateValue::Array(vec![].into()),
            StorageHashMap::new(),
            ContractMaintenanceAuthority::default(),
        )
    }

    fn state_with_circuit(name: &str) -> ContractState<InMemoryDB> {
        use midnight_onchain_runtime::state::ContractOperation;
        let mut state = empty_state();
        state.operations = state
            .operations
            .insert(entry_point(name), ContractOperation::new(None, None));
        state
    }

    #[test]
    fn validate_committee_enforces_non_empty_and_threshold_bounds() {
        let a = SigningKey::sample(rand::thread_rng()).verifying_key();
        let b = SigningKey::sample(rand::thread_rng()).verifying_key();
        let committee = vec![a.clone(), b];
        assert!(validate_committee(&committee, 2).is_ok());
        assert!(validate_committee(&committee, 1).is_ok());
        assert!(
            validate_committee(&[], 1).is_err(),
            "empty committee is ungovernable"
        );
        assert!(
            validate_committee(&committee[..1], 0).is_err(),
            "threshold 0 = anyone can govern"
        );
        assert!(
            validate_committee(&committee, 3).is_err(),
            "threshold above committee size can never be met"
        );
        assert!(
            validate_committee(&[a.clone(), a.clone()], 2).is_err(),
            "duplicate committee key collapses k-of-n to 1 signer"
        );
    }

    #[test]
    fn validate_signatures_enforces_distinct_inrange_verifying_quorum() {
        let k0 = SigningKey::sample(rand::thread_rng());
        let k1 = SigningKey::sample(rand::thread_rng());
        let committee = vec![k0.verifying_key(), k1.verifying_key()];

        let data = b"the bytes the committee signs";
        let make = |sigs: &[(u32, &SigningKey)]| -> Vec<(u32, Signature)> {
            sigs.iter()
                .map(|(i, k)| (*i, k.sign(&mut rand::thread_rng(), data)))
                .collect()
        };
        let check = |sigs: &[(u32, &SigningKey)], threshold| {
            validate_signatures(data, &make(sigs), &committee, threshold)
        };

        // 2-of-2, both valid and distinct.
        assert!(check(&[(0, &k0), (1, &k1)], 2).is_ok());
        // Under threshold.
        assert!(check(&[(0, &k0)], 2).is_err());
        // Duplicate committee index (would be NotNormalized on-chain).
        assert!(check(&[(0, &k0), (0, &k0)], 2).is_err());
        // Index outside the committee (KeyNotInCommittee).
        assert!(check(&[(5, &k0)], 1).is_err());
        // Wrong key at an index (committee[0] is k0, signed by k1).
        assert!(check(&[(0, &k1)], 1).is_err());
    }

    #[test]
    fn set_maintenance_authority_sets_committee_threshold_counter() {
        let a = SigningKey::sample(rand::thread_rng()).verifying_key();
        let b = SigningKey::sample(rand::thread_rng()).verifying_key();
        let committee = vec![a, b];

        let state = set_maintenance_authority(empty_state(), committee.clone(), 2);

        let authority = state.maintenance_authority;
        assert_eq!(
            authority.committee,
            committee
                .into_iter()
                .map(ContractMaintenanceVerifyingKey::Schnorr)
                .collect::<Vec<_>>(),
            "committee should be [a, b]"
        );
        assert_eq!(authority.threshold, 2);
        assert_eq!(authority.counter, 0, "counter starts at 0");
    }

    #[test]
    fn validate_single_insert_and_remove() {
        // insert: ok when absent, err when present
        assert!(validate_vk_sequence(&empty_state(), &[("increment", true)]).is_ok());
        assert!(
            validate_vk_sequence(&state_with_circuit("increment"), &[("increment", true)]).is_err(),
            "inserting an already-defined circuit should error"
        );
        // remove: ok when present, err when absent
        assert!(
            validate_vk_sequence(&state_with_circuit("increment"), &[("increment", false)]).is_ok()
        );
        assert!(
            validate_vk_sequence(&empty_state(), &[("increment", false)]).is_err(),
            "removing a non-existent circuit should error"
        );
    }

    #[test]
    fn validate_batch_simulates_effects_in_order() {
        let present = state_with_circuit("increment");
        // remove then insert the same circuit: valid as a batch (rotation).
        assert!(
            validate_vk_sequence(&present, &[("increment", false), ("increment", true)]).is_ok(),
            "remove-then-insert of the same circuit should be valid"
        );
        // insert then remove a fresh circuit: valid.
        assert!(validate_vk_sequence(&empty_state(), &[("new", true), ("new", false)]).is_ok());
        // inserting the same circuit twice: the second fails (now present).
        assert!(
            validate_vk_sequence(&empty_state(), &[("x", true), ("x", true)]).is_err(),
            "double insert should error on the second"
        );
        // removing twice: the second fails (now absent).
        assert!(
            validate_vk_sequence(&present, &[("increment", false), ("increment", false)]).is_err()
        );
    }
}
