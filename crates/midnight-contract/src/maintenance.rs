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
use std::sync::Arc;

use midnight_base_crypto::signatures::{Signature, SigningKey, VerifyingKey};
use midnight_bindgen::{ContractMaintenanceAuthority, ContractState, InMemoryDB};
use midnight_helpers::{
    ContractMaintenanceAuthority as LhAuthority, ContractOperationVersion,
    ContractOperationVersionedVerifierKey, DefaultDB, MaintenanceUpdate, SingleUpdate,
};
use midnight_onchain_runtime::state::EntryPointBuf;
use midnight_provider::{MidnightProvider, PendingTx, Provider};

use crate::Prover;
use crate::call::make_proof_provider;
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
        committee,
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

/// Validate that an update's attached signatures will satisfy `committee` /
/// `threshold` the way the ledger does: each signature's committee index is
/// in range and **distinct**, each verifies over `data_to_sign`, and the count
/// of distinct valid signatures meets the threshold. Turns on-chain rejections
/// (`NotNormalized` / `KeyNotInCommittee` / `InvalidCommitteeSignature` /
/// `ThresholdMissed`) into early, specific errors.
fn validate_signatures(
    update: &MaintenanceUpdate<DefaultDB>,
    committee: &[VerifyingKey],
    threshold: u32,
) -> Result<(), ContractError> {
    let data = update.data_to_sign();
    let mut seen = std::collections::HashSet::new();
    for sv in update.signatures.iter() {
        let (idx, sig) = sv.into_inner();
        let vk = committee.get(idx as usize).ok_or_else(|| {
            ContractError::Maintenance(format!(
                "signature index {idx} is outside the committee (size {})",
                committee.len()
            ))
        })?;
        if !seen.insert(idx) {
            return Err(ContractError::Maintenance(format!(
                "duplicate signature for committee index {idx}"
            )));
        }
        if !vk.verify(&data, &sig) {
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
// SingleUpdate construction (over the helpers' DefaultDB).
// ---------------------------------------------------------------------------

/// Parse the raw bytes of a compiled `*.verifier` key into the versioned form
/// the ledger expects for `VerifierKeyInsert`.
fn parse_versioned_verifier_key(
    bytes: &[u8],
) -> Result<ContractOperationVersionedVerifierKey, ContractError> {
    use midnight_transient_crypto::proofs::VerifierKey;
    let vk: VerifierKey = midnight_serialize::tagged_deserialize(&mut &bytes[..])
        .map_err(|e| ContractError::Maintenance(format!("invalid verifier key: {e}")))?;
    Ok(ContractOperationVersionedVerifierKey::V3(vk))
}

fn single_insert(circuit: &str, vk: ContractOperationVersionedVerifierKey) -> SingleUpdate {
    SingleUpdate::VerifierKeyInsert(circuit.as_bytes().into(), vk)
}

fn single_remove(circuit: &str) -> SingleUpdate {
    SingleUpdate::VerifierKeyRemove(circuit.as_bytes().into(), ContractOperationVersion::V3)
}

/// `ReplaceAuthority` installing `committee`/`threshold`. The new authority's
/// `counter` must be the current counter + 1 (a ledger well-formedness rule).
fn single_replace_authority(
    committee: Vec<VerifyingKey>,
    threshold: u32,
    current_counter: u32,
) -> SingleUpdate {
    SingleUpdate::ReplaceAuthority(LhAuthority {
        committee,
        threshold,
        // saturating to match the ledger's apply path (it caps at u32::MAX).
        counter: current_counter.saturating_add(1),
    })
}

// ---------------------------------------------------------------------------
// Balance / prove / submit a pre-signed maintenance update.
// ---------------------------------------------------------------------------

/// A [`BuildContractAction`](midnight_helpers::BuildContractAction) that attaches
/// an already-signed `MaintenanceUpdate` to the intent (no signing of its own).
struct AttachMaintenance {
    update: MaintenanceUpdate<DefaultDB>,
}

#[async_trait::async_trait]
impl midnight_helpers::BuildContractAction<DefaultDB> for AttachMaintenance {
    async fn build(
        &mut self,
        _rng: &mut midnight_helpers::StdRng,
        _context: Arc<midnight_helpers::LedgerContext<DefaultDB>>,
        intent: &midnight_helpers::Intent<
            midnight_helpers::Signature,
            midnight_helpers::ProofPreimageMarker,
            midnight_helpers::PedersenRandomness,
            DefaultDB,
        >,
    ) -> midnight_helpers::Intent<
        midnight_helpers::Signature,
        midnight_helpers::ProofPreimageMarker,
        midnight_helpers::PedersenRandomness,
        DefaultDB,
    > {
        intent.add_maintenance_update(self.update.clone())
    }
}

/// Balance, prove, and serialize a maintenance transaction carrying a pre-signed
/// `update`. Mirrors [`crate::deploy::deploy_funded`]: a maintenance update is
/// just another intent action with no ZK proof of its own, so it rides the same
/// dust-balancing pipeline.
async fn maintenance_funded(
    provider: &MidnightProvider,
    update: MaintenanceUpdate<DefaultDB>,
    prover: &Prover,
) -> Result<Vec<u8>, ContractError> {
    use midnight_helpers::{
        FromContext, IntentInfo, OfferInfo, ProofProvider, StandardTrasactionInfo,
    };

    let wallet_seed = provider.seed().await?;

    let context = provider.build_context().await?;

    // Maintenance updates contain no circuit calls, so a dust-only resolver
    // (no circuit proving keys) suffices.
    let resolver = crate::call::build_dust_only_resolver()?;
    context.update_resolver(resolver).await;

    let proof_provider: Arc<dyn ProofProvider<DefaultDB>> = make_proof_provider(prover);
    let reserved_at = context.latest_block_context().tblock;

    let intent_info: IntentInfo<DefaultDB> = IntentInfo {
        guaranteed_unshielded_offer: None,
        fallible_unshielded_offer: None,
        actions: vec![Box::new(AttachMaintenance { update })],
    };

    let mut tx_info = StandardTrasactionInfo::new_from_context(context, proof_provider, None);
    tx_info.add_intent(1, Box::new(intent_info));
    tx_info.set_guaranteed_offer(OfferInfo {
        inputs: vec![],
        outputs: vec![],
        transients: vec![],
    });
    tx_info.set_funding_seeds(vec![wallet_seed]);
    tx_info.use_mock_proofs_for_fees(true);

    let built = midnight_wallet::transfer::build_no_validate(tx_info)
        .await
        .map_err(|e| ContractError::Construction(format!("prove/balance failed: {e}")))?;

    if let Ok(mut wallet) = provider.wallet_mut().await {
        wallet.reserve_pending(built.dust_batches, Vec::new(), reserved_at);
    }

    let mut bytes = Vec::new();
    midnight_helpers::midnight_serialize::tagged_serialize(&built.finalized, &mut bytes)
        .map_err(|e| ContractError::Serialization(format!("{e}")))?;
    Ok(bytes)
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
    /// step in order), and build the unsigned [`MaintenanceUpdate`]. The returned
    /// [`PreparedMaintenance`] exposes the bytes each committee member signs.
    pub async fn prepare(self) -> Result<PreparedMaintenance<'a, P>, ContractError>
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
        let state = crate::state::fetch_state_from_node(provider, address_hex, None).await?;
        let counter = state.maintenance_authority.counter;
        let threshold = state.maintenance_authority.threshold;
        let committee = state.maintenance_authority.committee.clone();

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

        let mut singles = Vec::with_capacity(self.specs.len());
        for spec in self.specs {
            singles.push(match spec {
                OpSpec::Insert {
                    circuit,
                    verifier_key,
                } => single_insert(&circuit, parse_versioned_verifier_key(&verifier_key)?),
                OpSpec::Remove { circuit } => single_remove(&circuit),
                OpSpec::Replace {
                    committee,
                    threshold,
                } => single_replace_authority(committee, threshold, counter),
            });
        }

        let update = MaintenanceUpdate::new(address, singles, counter);
        Ok(PreparedMaintenance {
            contract: self.contract,
            update,
            committee,
            required_threshold: threshold,
        })
    }
}

enum OpSpec {
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
pub struct PreparedMaintenance<'a, P> {
    contract: &'a Contract<P>,
    update: MaintenanceUpdate<DefaultDB>,
    /// The on-chain committee at prepare time — used to verify attached
    /// signatures before submission.
    committee: Vec<VerifyingKey>,
    required_threshold: u32,
}

impl<'a, P> PreparedMaintenance<'a, P> {
    /// The exact bytes each committee member signs (with
    /// [`SigningKey::sign`](midnight_base_crypto::signatures::SigningKey::sign)).
    /// Distribute these to the members; collect their signatures via
    /// [`Self::add_signature`].
    pub fn data_to_sign(&self) -> Vec<u8> {
        self.update.data_to_sign()
    }

    /// Attach a signature produced (anywhere) over [`Self::data_to_sign`], at the
    /// signer's position in the on-chain committee.
    pub fn add_signature(mut self, committee_index: u32, signature: Signature) -> Self {
        self.update = self.update.add_signature(committee_index, signature);
        self
    }

    /// Convenience for the local case: sign [`Self::data_to_sign`] with `key` and
    /// attach it at `committee_index`.
    pub fn sign(self, committee_index: u32, key: &SigningKey) -> Self {
        let signature = key.sign(&mut rand::thread_rng(), &self.update.data_to_sign());
        self.add_signature(committee_index, signature)
    }

    /// Check the attached signatures against the committee/threshold captured at
    /// prepare time — distinct in-range indices, each verifying, count >=
    /// threshold — so an under-signed or malformed set fails here rather than
    /// after paying to build and submit.
    fn check_signatures(&self) -> Result<(), ContractError> {
        validate_signatures(&self.update, &self.committee, self.required_threshold)
    }

    /// Build, prove, and balance the transaction without submitting it. Errors if
    /// fewer than the authority threshold of signatures have been attached.
    pub async fn build(self) -> Result<Vec<u8>, ContractError>
    where
        P: Provider + AsMidnightProvider,
    {
        self.check_signatures()?;
        let provider = self.contract.provider().as_midnight_provider();
        maintenance_funded(provider, self.update, self.contract.prover()).await
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
            self.check_signatures()?;
            let provider = self.contract.provider().as_midnight_provider();
            let bytes = maintenance_funded(provider, self.update, self.contract.prover()).await?;
            Ok(provider.submit(&bytes).await?)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use midnight_bindgen::{StateValue, StorageHashMap};

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
            .insert(entry_point(name), ContractOperation::new(None));
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

        let make = |sigs: &[(u32, &SigningKey)]| -> MaintenanceUpdate<DefaultDB> {
            let addr = crate::address::parse_address(&"00".repeat(32)).unwrap();
            let mut u = MaintenanceUpdate::new(addr, vec![], 0);
            let data = u.data_to_sign();
            for (i, k) in sigs {
                u = u.add_signature(*i, k.sign(&mut rand::thread_rng(), &data));
            }
            u
        };

        // 2-of-2, both valid and distinct.
        assert!(validate_signatures(&make(&[(0, &k0), (1, &k1)]), &committee, 2).is_ok());
        // Under threshold.
        assert!(validate_signatures(&make(&[(0, &k0)]), &committee, 2).is_err());
        // Duplicate committee index (would be NotNormalized on-chain).
        assert!(validate_signatures(&make(&[(0, &k0), (0, &k0)]), &committee, 2).is_err());
        // Index outside the committee (KeyNotInCommittee).
        assert!(validate_signatures(&make(&[(5, &k0)]), &committee, 1).is_err());
        // Wrong key at an index (committee[0] is k0, signed by k1).
        assert!(validate_signatures(&make(&[(0, &k1)]), &committee, 1).is_err());
    }

    #[test]
    fn set_maintenance_authority_sets_committee_threshold_counter() {
        let a = SigningKey::sample(rand::thread_rng()).verifying_key();
        let b = SigningKey::sample(rand::thread_rng()).verifying_key();
        let committee = vec![a, b];

        let state = set_maintenance_authority(empty_state(), committee.clone(), 2);

        let authority = state.maintenance_authority;
        assert_eq!(authority.committee, committee, "committee should be [a, b]");
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

    #[test]
    fn remove_update_targets_the_named_circuit() {
        match single_remove("increment") {
            SingleUpdate::VerifierKeyRemove(ep, ver) => {
                assert_eq!(ep, "increment".as_bytes().into());
                assert_eq!(ver, ContractOperationVersion::V3);
            }
            _ => panic!("expected VerifierKeyRemove"),
        }
    }

    #[test]
    fn replace_authority_update_bumps_counter_and_sets_committee() {
        let a = SigningKey::sample(rand::thread_rng()).verifying_key();
        let b = SigningKey::sample(rand::thread_rng()).verifying_key();
        let committee = vec![a, b];
        match single_replace_authority(committee.clone(), 2, 5) {
            SingleUpdate::ReplaceAuthority(auth) => {
                assert_eq!(auth.counter, 6, "new authority counter must be current + 1");
                assert_eq!(auth.threshold, 2);
                assert_eq!(auth.committee, committee);
            }
            _ => panic!("expected ReplaceAuthority"),
        }
    }
}
