//! A contract maintenance update.

use std::collections::HashMap;
use std::sync::Arc;

use midnight_helpers::{
    BuildContext, ContractMaintenanceVerifyingKey, ContractOperationVersion,
    ContractOperationVersionedVerifierKey, DefaultDB, EntryPointBuf, LedgerState,
    MaintenanceUpdate, SingleUpdate, Timestamp, UnprovenIntent, WellFormedStrictness,
};
use midnight_provider::MidnightProvider;
use midnight_typed_state::{ContractState, InMemoryDB};
use midnight_types::convert::IntoLedger;
use midnight_types::{MidnightWallet, TransactionRequest, TxEnvelope};

use super::call::INTENT_SEGMENT;
use super::deploy::{assembled, with_single_action};

use crate::error::ContractError;
use crate::maintenance::OpSpec;

/// The unsigned update that applies `specs`, in order, to the contract at
/// `address`, whose state is `state`.
pub(crate) fn prepare_update(
    state: &ContractState<InMemoryDB>,
    address: midnight_types::ContractAddress,
    specs: &[OpSpec],
    counter: u32,
) -> Result<MaintenanceUpdate<DefaultDB>, ContractError> {
    // A removal names the slot its key is in. A key inserted earlier in the
    // same update is the one in that slot, whatever the chain holds.
    let mut inserted: HashMap<&str, midnight_helpers::ContractOperation> = HashMap::new();
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
                    Some(op) => midnight_helpers::contract_operation_version_of(op),
                    None => state
                        .operations
                        .get(&entry_point)
                        .map(|op| midnight_helpers::contract_operation_version_of(&op))
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
/// [`super::state::operation`] first.
fn versioned_verifier_key(
    bytes: &[u8],
) -> Result<ContractOperationVersionedVerifierKey, ContractError> {
    midnight_helpers::contract_operation_versioned_verifier_key(bytes.to_vec())
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
    committee: Vec<ContractMaintenanceVerifyingKey>,
    threshold: u32,
    current_counter: u32,
) -> SingleUpdate {
    SingleUpdate::ReplaceAuthority(midnight_helpers::ContractMaintenanceAuthority {
        committee,
        threshold,
        // saturating to match the ledger's apply path (it caps at u32::MAX).
        counter: current_counter.saturating_add(1),
    })
}

/// A [`BuildContractAction`](midnight_helpers::BuildContractAction) that attaches an
/// already-signed `MaintenanceUpdate` to the intent (no signing of its own).
struct AttachMaintenance {
    update: MaintenanceUpdate<DefaultDB>,
}

#[async_trait::async_trait]
impl midnight_helpers::BuildContractAction<DefaultDB, BuildContext> for AttachMaintenance {
    async fn build(
        &mut self,
        _rng: &mut midnight_helpers::StdRng,
        _context: Arc<BuildContext>,
        intent: &midnight_helpers::UnprovenIntent<DefaultDB>,
    ) -> midnight_helpers::UnprovenIntent<DefaultDB> {
        intent.add_maintenance_update(self.update.clone())
    }
}

/// Run the ledger's own check of the signed `update` (ordering, membership,
/// signatures, threshold, counter) against `state`, the contract's state, as a
/// node runs it before it takes the update.
pub(crate) fn check(
    update: &MaintenanceUpdate<DefaultDB>,
    state: &ContractState<InMemoryDB>,
) -> Result<(), ContractError> {
    // The maintenance check reads only the contract's authority, so a ledger
    // that holds just this contract is enough.
    let mut ledger = LedgerState::new("");
    ledger.contract = ledger.contract.insert(update.address, state.clone());
    UnprovenIntent::empty(&mut rand::thread_rng(), Timestamp::from_secs(0))
        .add_maintenance_update(update.clone())
        .well_formed(
            INTENT_SEGMENT,
            &ledger,
            WellFormedStrictness::default(),
            Timestamp::from_secs(0),
        )
        .map_err(|e| ContractError::Maintenance(format!("the ledger refuses the update: {e}")))
}

/// Balance, prove, and sign the transaction of the signed `update`. A
/// maintenance update is just another intent action with no ZK proof of its
/// own, so it rides the same dust-balancing pipeline as a deploy.
pub(crate) async fn maintenance_funded<W: MidnightWallet>(
    provider: &MidnightProvider<W>,
    update: MaintenanceUpdate<DefaultDB>,
) -> Result<TxEnvelope, ContractError> {
    let tx_info = with_single_action(
        provider.transaction_info().await?,
        AttachMaintenance { update },
    );
    Ok(provider
        .fill(TransactionRequest::contract(assembled(tx_info)))
        .await?)
}

#[cfg(test)]
mod tests {
    use midnight_typed_state::{ContractMaintenanceAuthority, StateValue, StorageHashMap};

    use super::*;
    use crate::{ContractMaintenanceSigningKey, ecdsa};

    #[test]
    fn check_runs_the_ledger_rules_against_the_contract_state() {
        let key = ecdsa::SigningKey::sample(rand::thread_rng());
        let member = ContractMaintenanceVerifyingKey::ECDSA(key.verifying_key());
        let key = ContractMaintenanceSigningKey::ECDSA(key);
        let outsider =
            ContractMaintenanceSigningKey::ECDSA(ecdsa::SigningKey::sample(rand::thread_rng()));
        let state = ContractState::new(
            StateValue::Array(vec![].into()),
            StorageHashMap::new(),
            ContractMaintenanceAuthority {
                committee: vec![member.clone()],
                threshold: 1,
                counter: 0,
            },
        );
        let address = crate::address::parse_contract_address(&"aa".repeat(32)).unwrap();
        let update = prepare_update(
            &state,
            address,
            &[OpSpec::Replace {
                committee: vec![member],
                threshold: 1,
            }],
            0,
        )
        .unwrap();
        let signed_by = |key: &ContractMaintenanceSigningKey| {
            let signature = key.sign(&mut rand::thread_rng(), &update.data_to_sign());
            update.clone().add_signature(0, signature)
        };

        assert!(check(&signed_by(&key), &state).is_ok());
        assert!(check(&signed_by(&outsider), &state).is_err());
    }
}
