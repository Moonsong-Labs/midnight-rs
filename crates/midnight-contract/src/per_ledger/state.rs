//! A contract's state in this generation's own types, as its transactions
//! carry it.

use std::io::Cursor;

use midnight_onchain_runtime::state::ContractMaintenanceVerifyingKey;
use midnight_serialize::{Tagged, peek_tag, tagged_deserialize, tagged_serialize};
use midnight_typed_state::{ContractState, InMemoryDB};

use super::helpers;
use helpers::{DefaultDB, EntryPointBuf};
use midnight_helpers::ContractVerifyingKeyBytes;

use crate::error::ContractError;
use crate::state::VerifierKeyVersion;

/// The contract state a call on this generation builds against, from the
/// bytes the chain served and the Compact side's view of them.
///
/// Bytes in this generation's encoding decode as they are. The view stands
/// in for bytes of another generation, which a chain still serves for states
/// from before its hard fork.
pub(crate) fn native_state(
    view: &ContractState<InMemoryDB>,
    bytes: &[u8],
) -> Result<helpers::ContractState<DefaultDB>, ContractError> {
    let tag = peek_tag(&mut Cursor::new(bytes))
        .map_err(|e| ContractError::Serialization(format!("contract state: {e}")))?;
    if tag == helpers::ContractState::<DefaultDB>::tag() {
        return tagged_deserialize(&mut &bytes[..])
            .map_err(|e| ContractError::Serialization(format!("deserialize state: {e}")));
    }
    from_view(view)
}

/// The Compact side's view of a contract state, in this generation's types.
///
/// A view in this generation's encoding converts as it is. Otherwise each
/// part converts on its own: the data by its encoding, which every
/// generation shares, each operation by its verifier key, and the committee
/// by its Schnorr keys. A view with a part this generation cannot hold, a key
/// it cannot verify or a committee member that is not Schnorr, is refused.
pub(crate) fn from_view(
    view: &ContractState<InMemoryDB>,
) -> Result<helpers::ContractState<DefaultDB>, ContractError> {
    if ContractState::<InMemoryDB>::tag() == helpers::ContractState::<DefaultDB>::tag() {
        return crate::call::reencode(view, "contract state");
    }

    let data: helpers::StateValue<DefaultDB> =
        crate::call::reencode(&*view.data.get(), "contract data")?;

    let mut operations = helpers::HashMapStorage::new();
    for entry in view.operations.iter() {
        let (entry_point, op) = &*entry;
        let circuit = String::from_utf8_lossy(&entry_point.0).into_owned();
        let key = match (&op.v2, &op.v3) {
            (Some(key), _) => Some(encoded(key, &circuit)?),
            (None, Some(key)) => Some(encoded(key, &circuit)?),
            (None, None) => None,
        };
        let native = match key {
            Some(key) => operation(&circuit, &key)?,
            None => helpers::contract_operation_new(None, None)
                .map_err(|e| ContractError::Construction(format!("operation {circuit}: {e}")))?,
        };
        operations = operations.insert(EntryPointBuf(entry_point.0.clone()), native);
    }

    let committee = view
        .maintenance_authority
        .committee
        .iter()
        .map(|member| match member {
            ContractMaintenanceVerifyingKey::Schnorr(key) => {
                Ok(helpers::maintenance_verifying_key(key.clone()))
            }
            _ => Err(ContractError::Construction(format!(
                "{} takes Schnorr maintenance committee keys only",
                super::LEDGER
            ))),
        })
        .collect::<Result<Vec<_>, _>>()?;
    let authority = helpers::ContractMaintenanceAuthority {
        committee,
        threshold: view.maintenance_authority.threshold,
        counter: view.maintenance_authority.counter,
    };

    Ok(helpers::ContractState::new(data, operations, authority))
}

fn encoded<T: midnight_serialize::Serializable + Tagged>(
    key: &T,
    circuit: &str,
) -> Result<Vec<u8>, ContractError> {
    let mut bytes = Vec::new();
    tagged_serialize(key, &mut bytes)
        .map_err(|e| ContractError::Serialization(format!("verifier key {circuit}: {e}")))?;
    Ok(bytes)
}

/// The operation that verifies `circuit`'s proofs against `verifier_key`, the
/// bytes of a compiled `*.verifier` artifact.
pub(crate) fn operation(
    circuit: &str,
    verifier_key: &[u8],
) -> Result<helpers::ContractOperation, ContractError> {
    accepted_key(circuit, verifier_key)?;
    helpers::contract_operation_new(Some(ContractVerifyingKeyBytes(verifier_key.to_vec())), None)
        .map_err(|e| ContractError::Construction(format!("verifier key {circuit}: {e}")))
}

/// Check that this generation verifies proofs against `verifier_key`.
///
/// The generation's own constructor can panic on a key that does not decode,
/// so this runs first.
pub(crate) fn accepted_key(circuit: &str, verifier_key: &[u8]) -> Result<(), ContractError> {
    let version = crate::state::verifier_key_version(verifier_key)
        .map_err(|e| ContractError::Construction(format!("verifier key {circuit}: {e}")))?;
    if version == VerifierKeyVersion::V7 && super::LEDGER < midnight_types::LedgerVersion::V9 {
        return Err(ContractError::Construction(format!(
            "verifier key {circuit} is a `verifier-key[v7]`, which {} cannot verify; \
             compile the contract for this chain's ledger",
            super::LEDGER
        )));
    }
    Ok(())
}
