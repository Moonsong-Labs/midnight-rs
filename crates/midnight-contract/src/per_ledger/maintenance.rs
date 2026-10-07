//! A contract maintenance update on this generation.

use std::collections::HashMap;
use std::sync::Arc;

use midnight_base_crypto::signatures::{Signature, VerifyingKey};
use midnight_typed_state::{ContractState, InMemoryDB};

use super::helpers;
use super::provider::Builds;
use super::types::convert::IntoLedger;
use helpers::{
    BuildContext, ContractOperationVersion, ContractOperationVersionedVerifierKey, DefaultDB,
    EntryPointBuf, FromContext, IntentInfo, MaintenanceUpdate, OfferInfo, SingleUpdate,
    StandardTransactionInfo,
};

use crate::error::ContractError;
use crate::maintenance::OpSpec;

/// The unsigned update that applies `specs`, in order, to the contract at
/// `address`, whose state the chain served as `state_bytes` (viewed as
/// `state`).
pub(crate) fn prepare_update(
    state: &ContractState<InMemoryDB>,
    state_bytes: &[u8],
    address: midnight_types::ContractAddress,
    specs: &[OpSpec],
    counter: u32,
) -> Result<MaintenanceUpdate<DefaultDB>, ContractError> {
    let native = super::state::native_state(state, state_bytes)?;
    // A removal names the slot its key is in. A key inserted earlier in the
    // same update is the one in that slot, whatever the chain holds.
    let mut inserted: HashMap<&str, helpers::ContractOperation> = HashMap::new();
    let mut singles = Vec::with_capacity(specs.len());
    for spec in specs {
        singles.push(match spec {
            OpSpec::Insert {
                circuit,
                verifier_key,
            } => {
                inserted.insert(circuit, super::state::operation(circuit, verifier_key)?);
                single_insert(circuit, versioned_verifier_key(verifier_key)?)
            }
            OpSpec::Remove { circuit } => {
                let entry_point: EntryPointBuf = circuit.as_bytes().into();
                let version = match inserted.get(circuit.as_str()) {
                    Some(op) => helpers::contract_operation_version_of(op),
                    None => native
                        .operations
                        .get(&entry_point)
                        .map(|op| helpers::contract_operation_version_of(&op))
                        .ok_or_else(|| {
                            ContractError::Maintenance(format!(
                                "circuit '{circuit}' has no verifier key to remove"
                            ))
                        })?,
                };
                single_remove(circuit, version)
            }
            OpSpec::Replace {
                committee,
                threshold,
            } => single_replace_authority(committee.clone(), *threshold, counter),
        });
    }
    Ok(MaintenanceUpdate::new(
        address.into_ledger(),
        singles,
        counter,
    ))
}

/// The raw bytes of a compiled `*.verifier` key, in the versioned form the
/// ledger expects for `VerifierKeyInsert`. Callers check the key with
/// [`super::state::accepted_key`] first.
fn versioned_verifier_key(
    bytes: &[u8],
) -> Result<ContractOperationVersionedVerifierKey, ContractError> {
    helpers::contract_operation_versioned_verifier_key(bytes.to_vec())
        .map_err(|e| ContractError::Maintenance(format!("invalid verifier key: {e}")))
}

fn single_insert(circuit: &str, vk: ContractOperationVersionedVerifierKey) -> SingleUpdate {
    SingleUpdate::VerifierKeyInsert(circuit.as_bytes().into(), vk)
}

fn single_remove(circuit: &str, version: ContractOperationVersion) -> SingleUpdate {
    SingleUpdate::VerifierKeyRemove(circuit.as_bytes().into(), version)
}

/// `ReplaceAuthority` installing `committee`/`threshold`. The new authority's
/// `counter` must be the current counter + 1 (a ledger well-formedness rule).
fn single_replace_authority(
    committee: Vec<VerifyingKey>,
    threshold: u32,
    current_counter: u32,
) -> SingleUpdate {
    SingleUpdate::ReplaceAuthority(helpers::ContractMaintenanceAuthority {
        committee: committee
            .into_iter()
            .map(helpers::maintenance_verifying_key)
            .collect(),
        threshold,
        // saturating to match the ledger's apply path (it caps at u32::MAX).
        counter: current_counter.saturating_add(1),
    })
}

/// A [`BuildContractAction`](helpers::BuildContractAction) that attaches an
/// already-signed `MaintenanceUpdate` to the intent (no signing of its own).
struct AttachMaintenance {
    update: MaintenanceUpdate<DefaultDB>,
}

#[async_trait::async_trait]
impl helpers::BuildContractAction<DefaultDB, BuildContext> for AttachMaintenance {
    async fn build(
        &mut self,
        _rng: &mut helpers::StdRng,
        _context: Arc<BuildContext>,
        intent: &helpers::Intent<
            helpers::Signature,
            helpers::ProofPreimageMarker,
            helpers::PedersenRandomness,
            DefaultDB,
        >,
    ) -> helpers::Intent<
        helpers::Signature,
        helpers::ProofPreimageMarker,
        helpers::PedersenRandomness,
        DefaultDB,
    > {
        intent.add_maintenance_update(self.update.clone())
    }
}

/// Sign `update` with `signatures`, then balance, prove, and serialize the
/// maintenance transaction. A maintenance update is just another intent
/// action with no ZK proof of its own, so it rides the same dust-balancing
/// pipeline as a deploy.
pub(crate) async fn maintenance_funded(
    builds: &Builds<'_>,
    update: MaintenanceUpdate<DefaultDB>,
    signatures: &[(u32, Signature)],
) -> Result<Vec<u8>, ContractError> {
    let update = signatures
        .iter()
        .fold(update, |update, (index, signature)| {
            update.add_signature(*index, helpers::transaction_signature(signature.clone()))
        });

    let context = builds.execution_context().await?;
    let intent_info: IntentInfo<DefaultDB, BuildContext> = IntentInfo {
        guaranteed_unshielded_offer: None,
        fallible_unshielded_offer: None,
        actions: vec![Box::new(AttachMaintenance { update })],
    };

    let mut tx_info =
        StandardTransactionInfo::new_from_context(context, builds.proof_provider(), None);
    tx_info.add_intent(1, Box::new(intent_info));
    tx_info.set_guaranteed_offer(OfferInfo {
        inputs: vec![],
        outputs: vec![],
        transients: vec![],
    });
    tx_info.use_mock_proofs_for_fees(true);

    let built = builds.build_funded(tx_info).await?;
    Ok(built.tx_bytes)
}

#[cfg(test)]
mod tests {
    use midnight_base_crypto::signatures::SigningKey;

    use super::*;

    #[test]
    fn replace_authority_update_bumps_counter_and_sets_committee() {
        let a = SigningKey::sample(rand::thread_rng()).verifying_key();
        let b = SigningKey::sample(rand::thread_rng()).verifying_key();
        let committee = vec![a, b];
        match single_replace_authority(committee.clone(), 2, 5) {
            SingleUpdate::ReplaceAuthority(auth) => {
                assert_eq!(auth.counter, 6, "new authority counter must be current + 1");
                assert_eq!(auth.threshold, 2);
                let expected: Vec<_> = committee
                    .into_iter()
                    .map(helpers::maintenance_verifying_key)
                    .collect();
                assert_eq!(auth.committee, expected);
            }
            _ => panic!("expected ReplaceAuthority"),
        }
    }
}
