//! Reading and preparing on-chain contract state.
//!
//! Two state-retrieval entry points are reachable from bindgen-generated code:
//!
//! - [`fetch_state`] goes through the indexer (latest state).
//! - [`fetch_state_from_node`] goes through the node's `midnight_contractState`
//!   RPC. Use this when you want a hash-pinned view or when the indexer hasn't
//!   caught up to the block yet.
//!
//! The other helpers in this module (`deserialize_state`, `verifier_keys`) are
//! `pub(crate)` plumbing used by `Contract::deploy`/`Contract::at`.

use midnight_typed_state::{ContractState, InMemoryDB};

use crate::error::ContractError;

/// Deserialize a hex-encoded contract state (as returned by the indexer or the
/// node RPC) into a [`ContractState`].
pub(crate) fn deserialize_state(
    hex_state: &str,
) -> Result<ContractState<InMemoryDB>, ContractError> {
    let bytes = hex::decode(hex_state)
        .map_err(|e| ContractError::StateFetch(format!("hex decode: {e}")))?;
    midnight_typed_state::decode_contract_state(&bytes)
        .map_err(|e| ContractError::StateFetch(format!("deserialize: {e}")))
}

/// Fetch contract state from a provider's indexer and deserialize it. Returns
/// [`ContractError::NotFound`] when the contract is missing from the indexer.
pub async fn fetch_state<P: midnight_provider::Provider>(
    provider: &P,
    address: &str,
) -> Result<ContractState<InMemoryDB>, ContractError> {
    let hex = provider
        .get_contract_state(address, None)
        .await
        .map_err(|e| ContractError::StateFetch(format!("provider: {e}")))?
        .ok_or_else(|| ContractError::NotFound(address.to_string()))?;
    deserialize_state(&hex)
}

/// Fetch contract state directly from the node RPC (`midnight_contractState`).
///
/// This uses the standard node RPC available on all devnet nodes, unlike
/// `midnight_queryContractState` which requires a custom node build. Pass a
/// block hash to pin the read to a specific block.
pub async fn fetch_state_from_node(
    provider: &midnight_provider::MidnightProvider,
    address: &str,
    at_block_hash: Option<midnight_provider::NodeBlockHash>,
) -> Result<ContractState<InMemoryDB>, ContractError> {
    Ok(node_state(provider, address, at_block_hash).await?.1)
}

/// [`fetch_state_from_node`], with the bytes the node served alongside the
/// view: a transaction carries the state in the chain's own encoding.
pub(crate) async fn node_state(
    provider: &midnight_provider::MidnightProvider,
    address: &str,
    at_block_hash: Option<midnight_provider::NodeBlockHash>,
) -> Result<(Vec<u8>, ContractState<InMemoryDB>), ContractError> {
    let hex = provider
        .get_state_from_node(address, at_block_hash)
        .await
        .map_err(|e| ContractError::StateFetch(format!("node RPC: {e}")))?
        .ok_or_else(|| ContractError::NotFound(address.to_string()))?;
    let bytes =
        hex::decode(&hex).map_err(|e| ContractError::StateFetch(format!("hex decode: {e}")))?;
    let view = midnight_typed_state::decode_contract_state(&bytes)
        .map_err(|e| ContractError::StateFetch(format!("deserialize: {e}")))?;
    Ok((bytes, view))
}

/// Load verifier keys from a [`ZkConfigProvider`] and insert them into the
/// contract state's operations map, keyed by circuit id. See
/// [`verifier_keys`].
///
/// [`ZkConfigProvider`]: crate::zk_config::ZkConfigProvider
pub(crate) fn populate_verifier_keys(
    mut state: ContractState<InMemoryDB>,
    zk_config: &dyn crate::zk_config::ZkConfigProvider,
    declared: Option<&[String]>,
) -> Result<ContractState<InMemoryDB>, ContractError> {
    for (circuit, key) in verifier_keys(zk_config, declared)? {
        // The Compact side's view is ledger 9's state, whose operation holds
        // either key version.
        let op = midnight_helpers::ledger_9::contract_operation_new(
            Some(midnight_helpers::ContractVerifyingKeyBytes(key)),
            None,
        )
        .map_err(|e| ContractError::Construction(format!("verifier key {circuit}: {e}")))?;
        state.operations = state.operations.insert(circuit.as_bytes().into(), op);
    }
    Ok(state)
}

/// The encoding of a verifier key, which decides the generations that can
/// verify proofs against it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VerifierKeyVersion {
    /// `verifier-key[v6]`, which every generation verifies.
    V6,
    /// `verifier-key[v7]`, from ledger 9 on.
    V7,
}

/// The encoding of the verifier key in `bytes`, which must decode in full.
pub(crate) fn verifier_key_version(bytes: &[u8]) -> Result<VerifierKeyVersion, String> {
    use midnight_helpers::{Tagged, ledger_8, ledger_9};
    use midnight_serialize::tagged_deserialize;
    let tag = midnight_serialize::peek_tag(&mut std::io::Cursor::new(bytes))
        .map_err(|e| format!("not a tagged verifier key: {e}"))?;
    let decoded = if tag == ledger_8::VerifierKey::tag() {
        tagged_deserialize::<ledger_8::VerifierKey>(&mut &bytes[..]).map(|_| VerifierKeyVersion::V6)
    } else if tag == ledger_9::VerifierKey::tag() {
        tagged_deserialize::<ledger_9::VerifierKey>(&mut &bytes[..]).map(|_| VerifierKeyVersion::V7)
    } else {
        return Err(format!(
            "`{tag}` is not a verifier key encoding this SDK knows"
        ));
    };
    decoded.map_err(|e| format!("`{tag}` does not decode: {e}"))
}

/// The verifier key of each circuit a deploy registers, as the bytes of its
/// compiled `*.verifier` artifact, keyed by circuit id (e.g. the `increment`
/// circuit → entry point `"increment"`).
///
/// Required for on-chain deployment — without verifier keys, the node cannot
/// verify ZK proofs for circuit calls. The provider must be able to enumerate
/// its circuits ([`ZkConfigProvider::list_circuits`]); a provider that cannot
/// (returns `None`) can drive calls but not a deploy.
///
/// [`ZkConfigProvider::list_circuits`]: crate::zk_config::ZkConfigProvider::list_circuits
pub(crate) fn verifier_keys(
    zk_config: &dyn crate::zk_config::ZkConfigProvider,
    declared: Option<&[String]>,
) -> Result<Vec<(String, Vec<u8>)>, ContractError> {
    let enumerated = zk_config
        .list_circuits()
        .map_err(|e| ContractError::Construction(format!("listing circuits: {e}")))?;

    // With a declared set in hand the compiled contract is the source of truth
    // and the provider only supplies bytes; the enumeration is then just a
    // cross-check that catches stale and missing artifacts. Without one (the
    // hand-written `Contract::deploy` path, or a provider that cannot
    // enumerate) fall back to whatever the provider reports.
    let circuits = match (declared, enumerated) {
        (Some(declared), enumerated) => {
            if let Some(found) = enumerated {
                let mut stale: Vec<&str> = found
                    .iter()
                    .map(String::as_str)
                    .filter(|c| !declared.iter().any(|d| d == c))
                    .collect();
                if !stale.is_empty() {
                    stale.sort_unstable();
                    return Err(ContractError::Construction(format!(
                        "zk artifacts contain verifier keys for circuits the contract does not \
                         declare: {}; the artifact directory is stale, rebuild it",
                        stale.join(", ")
                    )));
                }
            }
            declared.to_vec()
        }
        (None, Some(found)) => found,
        (None, None) => {
            return Err(ContractError::Construction(
                "zk config provider cannot enumerate circuits; deploy requires an enumerable \
                 provider (e.g. FsZkConfigProvider) or a contract that declares its circuits"
                    .into(),
            ));
        }
    };

    circuits
        .into_iter()
        .map(|circuit| {
            let bytes = zk_config
                .verifier_key(&circuit)
                .map_err(|e| ContractError::Construction(format!("verifier key {circuit}: {e}")))?;
            verifier_key_version(&bytes).map_err(|e| {
                ContractError::Construction(format!("deserialize {circuit}.verifier: {e}"))
            })?;
            Ok((circuit, bytes))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counter_compiled_dir() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../devnet/contracts/counter/compiled")
    }

    #[test]
    fn verifier_keys_loads_increment() {
        let provider = crate::zk_config::FsZkConfigProvider::new(counter_compiled_dir());
        let keys = verifier_keys(&provider, None).unwrap();

        let (_, key) = keys
            .iter()
            .find(|(circuit, _)| circuit == "increment")
            .expect("increment verifier key");
        assert_eq!(verifier_key_version(key), Ok(VerifierKeyVersion::V6));
    }

    /// The generations' own constructors panic on a key that does not decode,
    /// and this check is what keeps a bad artifact away from them.
    #[test]
    fn a_verifier_key_that_does_not_decode_is_refused() {
        let key = std::fs::read(counter_compiled_dir().join("keys/increment.verifier")).unwrap();
        assert!(verifier_key_version(&key[..key.len() - 1]).is_err());
    }

    /// A mistyped `with_zk_config` path used to enumerate zero circuits and
    /// deploy a contract with an empty operations map: funds spent, every call
    /// rejected, recoverable only by a maintenance update.
    #[test]
    fn missing_keys_directory_is_an_error_not_an_empty_deploy() {
        let provider = crate::zk_config::FsZkConfigProvider::new("/nonexistent/compiledd");
        let err = verifier_keys(&provider, None)
            .expect_err("a nonexistent artifact directory must not deploy silently");
        let msg = err.to_string();
        assert!(
            msg.contains("compiledd"),
            "error should name the offending path, got: {msg}"
        );
    }

    /// The filesystem supplies the bytes; the compiled contract is the source
    /// of truth for which circuits exist.
    #[test]
    fn declared_circuit_without_a_key_file_is_rejected() {
        let provider = crate::zk_config::FsZkConfigProvider::new(counter_compiled_dir());
        let declared = vec![
            "increment".to_string(),
            "increment_by".to_string(),
            "decrement".to_string(),
        ];
        let err = verifier_keys(&provider, Some(&declared))
            .expect_err("a declared circuit with no verifier key must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("decrement"),
            "error should name the missing circuit, got: {msg}"
        );
    }

    /// A stale `.verifier` file left in `keys/` would otherwise register a
    /// bogus entry point on the deployed contract.
    #[test]
    fn key_file_not_declared_by_the_contract_is_rejected() {
        let provider = crate::zk_config::FsZkConfigProvider::new(counter_compiled_dir());
        let declared = vec!["increment".to_string()];
        let err = verifier_keys(&provider, Some(&declared))
            .expect_err("an undeclared key file must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("increment_by"),
            "error should name the stale artifact, got: {msg}"
        );
    }

    #[test]
    fn declared_set_matching_the_directory_populates_every_circuit() {
        let provider = crate::zk_config::FsZkConfigProvider::new(counter_compiled_dir());
        let declared = vec!["increment".to_string(), "increment_by".to_string()];
        let keys = verifier_keys(&provider, Some(&declared)).unwrap();
        for circuit in &declared {
            assert!(
                keys.iter().any(|(c, _)| c == circuit),
                "{circuit} should be registered"
            );
        }
    }
}
