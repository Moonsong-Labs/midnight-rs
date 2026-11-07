//! A circuit's operation, from the verifier key of its compiled artifact.

use midnight_helpers::ContractVerifyingKeyBytes;
use midnight_serialize::peek_tag;

use super::helpers;

use crate::error::ContractError;

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

/// Check that `verifier_key` is a `verifier-key[v6]` or a `verifier-key[v7]`,
/// the two versions that ledger 9 verifies proofs against.
fn accepted_key(circuit: &str, verifier_key: &[u8]) -> Result<(), ContractError> {
    let tag = peek_tag(&mut std::io::Cursor::new(verifier_key)).map_err(|e| {
        ContractError::Construction(format!(
            "verifier key {circuit}: not a tagged verifier key: {e}"
        ))
    })?;
    // Refuse any other tag here: upstream `contract_operation_new` and
    // `contract_operation_versioned_verifier_key` panic on it.
    match tag.as_str() {
        "verifier-key[v6]" | "verifier-key[v7]" => Ok(()),
        _ => Err(ContractError::Construction(format!(
            "verifier key {circuit}: `{tag}` is not a verifier key encoding this SDK knows"
        ))),
    }
}
