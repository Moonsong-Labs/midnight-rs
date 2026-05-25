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

// ---------------------------------------------------------------------------
// Preconditions, checked against the fetched on-chain state.
// ---------------------------------------------------------------------------

/// Entry-point key for a circuit name, as stored in `ContractState.operations`.
fn entry_point(circuit: &str) -> EntryPointBuf {
    circuit.as_bytes().into()
}

/// Insert precondition: the circuit must not already have a verifier key.
/// (`VerifierKeyInsert` does not replace; you must remove first.)
fn ensure_not_defined(
    state: &ContractState<InMemoryDB>,
    circuit: &str,
) -> Result<(), ContractError> {
    if state.operations.contains_key(&entry_point(circuit)) {
        return Err(ContractError::Maintenance(format!(
            "circuit '{circuit}' already has a verifier key; remove it before inserting"
        )));
    }
    Ok(())
}

/// Remove precondition: the circuit must currently have a verifier key.
fn ensure_defined(state: &ContractState<InMemoryDB>, circuit: &str) -> Result<(), ContractError> {
    if !state.operations.contains_key(&entry_point(circuit)) {
        return Err(ContractError::Maintenance(format!(
            "circuit '{circuit}' has no verifier key to remove"
        )));
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
        counter: current_counter + 1,
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

    let wallet_seed = provider
        .seed()
        .await
        .map_err(|_| ContractError::Construction("provider has no wallet".into()))?;

    let context = provider
        .build_context()
        .await
        .map_err(|e| ContractError::Construction(format!("build context: {e}")))?;

    // Maintenance updates contain no circuit calls, so a dust-only resolver
    // (no circuit proving keys) suffices.
    let resolver = crate::deploy::make_deploy_resolver()?;
    context.update_resolver(Arc::new(resolver)).await;

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

/// Maintenance / governance operations for a deployed contract. Obtained via
/// [`Contract::maintenance`](crate::Contract::maintenance).
pub struct ContractMaintenance<'a, P> {
    contract: &'a Contract<P>,
}

impl<'a, P> ContractMaintenance<'a, P> {
    pub(crate) fn new(contract: &'a Contract<P>) -> Self {
        Self { contract }
    }

    /// Insert a verifier key for `circuit`. Fails (at `prepare`) if the circuit
    /// already has one. `verifier_key` is the raw bytes of a compiled
    /// `*.verifier` artifact.
    pub fn insert_verifier_key(
        &self,
        circuit: impl Into<String>,
        verifier_key: impl Into<Vec<u8>>,
    ) -> MaintenanceOp<'a, P> {
        MaintenanceOp {
            contract: self.contract,
            spec: OpSpec::Insert {
                circuit: circuit.into(),
                verifier_key: verifier_key.into(),
            },
        }
    }

    /// Remove the verifier key for `circuit`. Fails (at `prepare`) if absent.
    pub fn remove_verifier_key(&self, circuit: impl Into<String>) -> MaintenanceOp<'a, P> {
        MaintenanceOp {
            contract: self.contract,
            spec: OpSpec::Remove {
                circuit: circuit.into(),
            },
        }
    }

    /// Replace the maintenance authority with a new `committee`/`threshold`.
    pub fn replace_authority(
        &self,
        committee: Vec<VerifyingKey>,
        threshold: u32,
    ) -> MaintenanceOp<'a, P> {
        MaintenanceOp {
            contract: self.contract,
            spec: OpSpec::Replace {
                committee,
                threshold,
            },
        }
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

/// A pending maintenance operation. Call [`Self::prepare`] to fetch the current
/// authority state and build the (unsigned) update.
pub struct MaintenanceOp<'a, P> {
    contract: &'a Contract<P>,
    spec: OpSpec,
}

impl<'a, P> MaintenanceOp<'a, P>
where
    P: Provider + AsMidnightProvider,
{
    /// Fetch the current authority state, run the precondition check, and build
    /// the unsigned [`MaintenanceUpdate`]. The returned [`PreparedMaintenance`]
    /// exposes the bytes each committee member must sign.
    pub async fn prepare(self) -> Result<PreparedMaintenance<'a, P>, ContractError> {
        let provider = self.contract.provider().as_midnight_provider();
        let address_hex = self.contract.address();
        let address = crate::address::parse_address(address_hex)?;

        let state = crate::state::fetch_state_from_node(provider, address_hex, None).await?;
        let counter = state.maintenance_authority.counter;
        let threshold = state.maintenance_authority.threshold;

        let single = match self.spec {
            OpSpec::Insert {
                circuit,
                verifier_key,
            } => {
                ensure_not_defined(&state, &circuit)?;
                single_insert(&circuit, parse_versioned_verifier_key(&verifier_key)?)
            }
            OpSpec::Remove { circuit } => {
                ensure_defined(&state, &circuit)?;
                single_remove(&circuit)
            }
            OpSpec::Replace {
                committee,
                threshold,
            } => single_replace_authority(committee, threshold, counter),
        };

        let update = MaintenanceUpdate::new(address, vec![single], counter);
        Ok(PreparedMaintenance {
            contract: self.contract,
            update,
            required_threshold: threshold,
        })
    }
}

/// An unsigned (or partially-signed) maintenance update. Collect the committee
/// signatures with [`Self::sign`] / [`Self::add_signature`], then `.await`
/// (build + submit → [`PendingTx`]) or [`Self::build`] (proven bytes only).
pub struct PreparedMaintenance<'a, P> {
    contract: &'a Contract<P>,
    update: MaintenanceUpdate<DefaultDB>,
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

    fn ensure_threshold(&self) -> Result<(), ContractError> {
        let have = self.update.signatures.len();
        if (have as u32) < self.required_threshold {
            return Err(ContractError::Maintenance(format!(
                "not enough signatures: have {have}, authority threshold is {}",
                self.required_threshold
            )));
        }
        Ok(())
    }

    /// Build, prove, and balance the transaction without submitting it. Errors if
    /// fewer than the authority threshold of signatures have been attached.
    pub async fn build(self) -> Result<Vec<u8>, ContractError>
    where
        P: Provider + AsMidnightProvider,
    {
        self.ensure_threshold()?;
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
            self.ensure_threshold()?;
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
    fn ensure_not_defined_ok_when_absent_err_when_present() {
        assert!(ensure_not_defined(&empty_state(), "increment").is_ok());
        assert!(
            ensure_not_defined(&state_with_circuit("increment"), "increment").is_err(),
            "inserting an already-defined circuit should error"
        );
    }

    #[test]
    fn ensure_defined_ok_when_present_err_when_absent() {
        assert!(ensure_defined(&state_with_circuit("increment"), "increment").is_ok());
        assert!(
            ensure_defined(&empty_state(), "increment").is_err(),
            "removing a non-existent circuit should error"
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
