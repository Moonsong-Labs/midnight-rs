//! Contract maintenance / governance.
//!
//! A contract's on-chain `maintenance_authority` controls who may rotate its
//! verifier keys or hand control to another authority. See
//! `docs/contract-maintenance-governance.md` for the protocol model.

use std::sync::Arc;

use midnight_base_crypto::signatures::SigningKey;
use midnight_bindgen::{ContractMaintenanceAuthority, ContractState, InMemoryDB};
use midnight_onchain_runtime::state::EntryPointBuf;

use crate::call::make_proof_provider;
use crate::error::ContractError;

/// Entry-point key for a circuit name, as stored in `ContractState.operations`.
fn entry_point(circuit: &str) -> EntryPointBuf {
    circuit.as_bytes().into()
}

/// Serialize a maintenance signing key for storage in the private-state
/// provider. Paired with [`signing_key_from_bytes`].
pub(crate) fn signing_key_to_bytes(key: &SigningKey) -> Vec<u8> {
    let mut buf = Vec::new();
    midnight_serialize::Serializable::serialize(key, &mut buf)
        .expect("in-memory serialization is infallible");
    buf
}

/// Reconstruct a signing key stored via [`signing_key_to_bytes`].
pub(crate) fn signing_key_from_bytes(bytes: &[u8]) -> Result<SigningKey, ContractError> {
    SigningKey::from_bytes(bytes).map_err(|e| {
        ContractError::Maintenance(format!("stored maintenance signing key invalid: {e}"))
    })
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
// MaintenanceUpdateInfo construction (fed to the helpers' balance/prove path).
// ---------------------------------------------------------------------------

use midnight_helpers::{
    ContractMaintenanceAuthorityInfo, ContractOperationVersionedVerifierKey, MaintenanceUpdateInfo,
    UpdateInfo,
};

/// A `VerifierKeyRemove` update for `circuit`.
fn remove_update(circuit: &str) -> UpdateInfo {
    UpdateInfo::VerifierKeyRemove(circuit.as_bytes().into())
}

/// A `VerifierKeyInsert` update for `circuit`.
fn insert_update(circuit: &str, vk: ContractOperationVersionedVerifierKey) -> UpdateInfo {
    UpdateInfo::VerifierKeyInsert(circuit.as_bytes().into(), vk)
}

/// A `ReplaceAuthority` update installing a fresh 1-of-1 committee from
/// `new_key`. The new authority's `counter` must be the current counter + 1
/// (a ledger well-formedness rule).
fn replace_authority_update(new_key: &SigningKey, current_counter: u32) -> UpdateInfo {
    UpdateInfo::ReplaceAuthority(ContractMaintenanceAuthorityInfo {
        new_committee: vec![new_key.clone()],
        threshold: 1,
        counter: current_counter + 1,
    })
}

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

/// Balance, prove, and serialize a maintenance transaction carrying
/// `update_info`. Mirrors [`crate::deploy::deploy_funded`]: a maintenance
/// update is just another intent action, with no ZK proof of its own, so it
/// rides the same dust-balancing pipeline.
pub(crate) async fn maintenance_funded(
    provider: &midnight_provider::MidnightProvider,
    update_info: MaintenanceUpdateInfo,
    prover: &crate::Prover,
) -> Result<Vec<u8>, ContractError> {
    use midnight_helpers::{
        DefaultDB, FromContext, IntentInfo, OfferInfo, ProofProvider, StandardTrasactionInfo,
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
        actions: vec![Box::new(update_info)],
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

/// Assemble the signed-update descriptor: the current authority's key signs at
/// committee index 0, over an update carrying the current authority `counter`.
fn build_update_info(
    address: midnight_coin_structure::contract::ContractAddress,
    signer: &SigningKey,
    current_counter: u32,
    updates: Vec<UpdateInfo>,
) -> MaintenanceUpdateInfo {
    MaintenanceUpdateInfo {
        address,
        committee: vec![signer.clone()],
        updates,
        counter: current_counter,
    }
}

/// Set the contract's maintenance authority to a single-key (1-of-1) committee
/// derived from `signing_key`: `committee = [vk]`, `threshold = 1`,
/// `counter = 0`.
///
/// This is what makes a freshly-built deploy state governable. The default
/// authority produced by codegen has an empty committee, which can never
/// authorize a maintenance update.
pub(crate) fn set_maintenance_authority(
    mut state: ContractState<InMemoryDB>,
    signing_key: &SigningKey,
) -> ContractState<InMemoryDB> {
    state.maintenance_authority = ContractMaintenanceAuthority {
        committee: vec![signing_key.verifying_key()],
        threshold: 1,
        counter: 0,
    };
    state
}

// ---------------------------------------------------------------------------
// Public API: Contract::at(..).maintenance() sub-builder.
// ---------------------------------------------------------------------------

use std::future::{Future, IntoFuture};
use std::pin::Pin;

use crate::Contract;
use crate::contract::AsMidnightProvider;
use midnight_provider::{PendingTx, Provider};

/// Maintenance / governance operations for a deployed contract.
///
/// Obtained via [`Contract::maintenance`]. Each method returns a
/// [`MaintenanceTx`] that follows the repo's builder idiom: `.await` builds,
/// signs, and submits (returning a [`PendingTx`]); `.build().await` returns the
/// proven transaction bytes without submitting.
pub struct ContractMaintenance<'a, P> {
    contract: &'a Contract<P>,
}

impl<'a, P> ContractMaintenance<'a, P> {
    pub(crate) fn new(contract: &'a Contract<P>) -> Self {
        Self { contract }
    }

    /// Insert a verifier key for `circuit`. Fails if the circuit already has a
    /// key (you must [`Self::remove_verifier_key`] first). `verifier_key` is the
    /// raw bytes of a compiled `*.verifier` artifact.
    pub fn insert_verifier_key(
        &self,
        circuit: impl Into<String>,
        verifier_key: impl Into<Vec<u8>>,
    ) -> MaintenanceTx<'a, P> {
        MaintenanceTx {
            contract: self.contract,
            op: MaintenanceOp::Insert {
                circuit: circuit.into(),
                verifier_key: verifier_key.into(),
            },
        }
    }

    /// Remove the verifier key for `circuit`. Fails if the circuit has no key.
    pub fn remove_verifier_key(&self, circuit: impl Into<String>) -> MaintenanceTx<'a, P> {
        MaintenanceTx {
            contract: self.contract,
            op: MaintenanceOp::Remove {
                circuit: circuit.into(),
            },
        }
    }

    /// Replace the maintenance authority with a fresh 1-of-1 committee derived
    /// from `new_key`. On a successful submit the stored signing key is rewritten
    /// to `new_key`.
    pub fn replace_authority(&self, new_key: SigningKey) -> MaintenanceTx<'a, P> {
        MaintenanceTx {
            contract: self.contract,
            op: MaintenanceOp::ReplaceAuthority { new_key },
        }
    }
}

enum MaintenanceOp {
    Insert {
        circuit: String,
        verifier_key: Vec<u8>,
    },
    Remove {
        circuit: String,
    },
    ReplaceAuthority {
        new_key: SigningKey,
    },
}

/// A prepared maintenance transaction. See [`ContractMaintenance`] for the
/// `.await` (build + submit) vs `.build().await` (bytes only) distinction.
pub struct MaintenanceTx<'a, P> {
    contract: &'a Contract<P>,
    op: MaintenanceOp,
}

impl<'a, P> MaintenanceTx<'a, P>
where
    P: Provider + AsMidnightProvider,
{
    /// Load the signing key, read the current authority counter, run
    /// preconditions, and balance/prove the transaction. Returns the proven tx
    /// bytes and, for `replace_authority`, the new key bytes to persist on a
    /// successful submit.
    async fn prepare(&self) -> Result<(Vec<u8>, Option<Vec<u8>>), ContractError> {
        let provider = self.contract.provider().as_midnight_provider();
        let address_hex = self.contract.address();
        let address = crate::address::parse_address(address_hex)?;

        let store = provider.private_state().ok_or_else(|| {
            ContractError::Maintenance(
                "no private-state store configured; maintenance needs the signing key (call \
                 MidnightProvider::with_private_state)"
                    .into(),
            )
        })?;
        let key_bytes = store.get_signing_key(address_hex).await?.ok_or_else(|| {
            ContractError::Maintenance(format!(
                "no maintenance signing key stored for contract {address_hex}"
            ))
        })?;
        let signer = signing_key_from_bytes(&key_bytes)?;

        let state = crate::state::fetch_state_from_node(provider, address_hex, None).await?;
        let current_counter = state.maintenance_authority.counter;

        let (update, new_key) = match &self.op {
            MaintenanceOp::Insert {
                circuit,
                verifier_key,
            } => {
                ensure_not_defined(&state, circuit)?;
                let vk = parse_versioned_verifier_key(verifier_key)?;
                (insert_update(circuit, vk), None)
            }
            MaintenanceOp::Remove { circuit } => {
                ensure_defined(&state, circuit)?;
                (remove_update(circuit), None)
            }
            MaintenanceOp::ReplaceAuthority { new_key } => (
                replace_authority_update(new_key, current_counter),
                Some(signing_key_to_bytes(new_key)),
            ),
        };

        let update_info = build_update_info(address, &signer, current_counter, vec![update]);
        let tx_bytes = maintenance_funded(provider, update_info, self.contract.prover()).await?;
        Ok((tx_bytes, new_key))
    }

    /// Build, sign, and balance the transaction without submitting it. Returns
    /// the proven transaction bytes. Does not rewrite the stored signing key.
    pub async fn build(self) -> Result<Vec<u8>, ContractError> {
        Ok(self.prepare().await?.0)
    }
}

impl<'a, P> IntoFuture for MaintenanceTx<'a, P>
where
    P: Provider + AsMidnightProvider + Send + Sync + 'a,
{
    type Output = Result<PendingTx, ContractError>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send + 'a>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let provider = self.contract.provider().as_midnight_provider();
            let address_hex = self.contract.address().to_string();
            let (tx_bytes, new_key) = self.prepare().await?;
            let pending = provider.submit(&tx_bytes).await?;
            // Persist the rotated authority key after a successful submit. Note:
            // submit success precedes on-chain inclusion, so a tx that is later
            // dropped leaves the stored key ahead of the chain (same desync
            // window midnight-js documents for replace-authority).
            if let Some(new_key) = new_key {
                if let Some(store) = provider.private_state() {
                    store.set_signing_key(&address_hex, &new_key).await?;
                }
            }
            Ok(pending)
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
    fn sets_single_key_committee_threshold_one_counter_zero() {
        let key = SigningKey::sample(rand::thread_rng());
        let vk = key.verifying_key();

        let state = set_maintenance_authority(empty_state(), &key);

        let authority = state.maintenance_authority;
        assert_eq!(authority.committee, vec![vk], "committee should be [vk]");
        assert_eq!(authority.threshold, 1, "threshold should be 1");
        assert_eq!(authority.counter, 0, "counter should start at 0");
    }

    #[test]
    fn ensure_not_defined_ok_when_absent_err_when_present() {
        let absent = empty_state();
        assert!(ensure_not_defined(&absent, "increment").is_ok());

        let present = state_with_circuit("increment");
        assert!(
            ensure_not_defined(&present, "increment").is_err(),
            "inserting an already-defined circuit should error"
        );
    }

    #[test]
    fn ensure_defined_ok_when_present_err_when_absent() {
        let present = state_with_circuit("increment");
        assert!(ensure_defined(&present, "increment").is_ok());

        let absent = empty_state();
        assert!(
            ensure_defined(&absent, "increment").is_err(),
            "removing a non-existent circuit should error"
        );
    }

    #[test]
    fn remove_update_targets_the_named_circuit() {
        match remove_update("increment") {
            UpdateInfo::VerifierKeyRemove(ep) => {
                assert_eq!(ep, "increment".as_bytes().into());
            }
            _ => panic!("expected VerifierKeyRemove"),
        }
    }

    #[test]
    fn replace_authority_update_bumps_counter_and_uses_new_key() {
        let new_key = SigningKey::sample(rand::thread_rng());
        match replace_authority_update(&new_key, 5) {
            UpdateInfo::ReplaceAuthority(info) => {
                assert_eq!(info.counter, 6, "new authority counter must be current + 1");
                assert_eq!(info.threshold, 1);
                assert_eq!(info.new_committee.len(), 1);
                assert_eq!(
                    info.new_committee[0].verifying_key(),
                    new_key.verifying_key()
                );
            }
            _ => panic!("expected ReplaceAuthority"),
        }
    }

    #[test]
    fn signing_key_round_trips_through_bytes() {
        let key = SigningKey::sample(rand::thread_rng());
        let bytes = signing_key_to_bytes(&key);
        let restored = signing_key_from_bytes(&bytes).expect("should restore");
        assert_eq!(
            restored.verifying_key(),
            key.verifying_key(),
            "round-tripped key must yield the same verifying key"
        );
    }

    #[test]
    fn build_update_info_carries_current_counter_and_signer() {
        let signer = SigningKey::sample(rand::thread_rng());
        let addr = crate::address::parse_address(&"00".repeat(32)).unwrap();
        let info = build_update_info(addr, &signer, 7, vec![remove_update("foo")]);
        assert_eq!(
            info.counter, 7,
            "update counter must equal current authority counter"
        );
        assert_eq!(info.committee.len(), 1);
        assert_eq!(info.committee[0].verifying_key(), signer.verifying_key());
        assert_eq!(info.updates.len(), 1);
    }
}
