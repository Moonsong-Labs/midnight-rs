//! Reading and preparing on-chain contract state.
//!
//! Two state-retrieval entry points are reachable from bindgen-generated code:
//!
//! - [`fetch_state`] goes through the indexer (latest state).
//! - [`fetch_state_from_node`] goes through the node's `midnight_contractState`
//!   RPC. Use this when you want a hash-pinned view or when the indexer hasn't
//!   caught up to the block yet.
//!
//! The other helpers in this module (`deserialize_state`,
//! `populate_verifier_keys`) are `pub(crate)` plumbing used by
//! `Contract::deploy`/`Contract::at`.

use midnight_onchain_runtime::state::{ContractOperation, EntryPointBuf};
use midnight_typed_state::{ContractState, InMemoryDB};

use crate::error::ContractError;

/// Deserialize a hex-encoded contract state (as returned by the indexer or the
/// node RPC) into a [`ContractState`].
pub(crate) fn deserialize_state(
    hex_state: &str,
) -> Result<ContractState<InMemoryDB>, ContractError> {
    let bytes = hex::decode(hex_state)
        .map_err(|e| ContractError::StateFetch(format!("hex decode: {e}")))?;
    midnight_serialize::tagged_deserialize(&mut bytes.as_slice())
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
    let hex = provider
        .get_state_from_node(address, at_block_hash)
        .await
        .map_err(|e| ContractError::StateFetch(format!("node RPC: {e}")))?
        .ok_or_else(|| ContractError::NotFound(address.to_string()))?;
    deserialize_state(&hex)
}

/// Load verifier keys from a [`ZkConfigProvider`] and insert them into the
/// contract state's operations map, keyed by circuit id (e.g. the `increment`
/// circuit → entry point `"increment"`).
///
/// Required for on-chain deployment — without verifier keys, the node cannot
/// verify ZK proofs for circuit calls. The provider must be able to enumerate
/// its circuits ([`ZkConfigProvider::list_circuits`]); a provider that cannot
/// (returns `None`) can drive calls but not a deploy.
pub(crate) fn populate_verifier_keys(
    mut state: ContractState<InMemoryDB>,
    zk_config: &dyn crate::zk_config::ZkConfigProvider,
    declared: Option<&[String]>,
) -> Result<ContractState<InMemoryDB>, ContractError> {
    use midnight_transient_crypto::proofs::VerifierKey;

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

    for circuit in circuits {
        let bytes = zk_config
            .verifier_key(&circuit)
            .map_err(|e| ContractError::Construction(format!("verifier key {circuit}: {e}")))?;

        let vk: VerifierKey = midnight_serialize::tagged_deserialize(&mut bytes.as_slice())
            .map_err(|e| {
                ContractError::Construction(format!("deserialize {circuit}.verifier: {e}"))
            })?;

        let entry_point: EntryPointBuf = circuit.as_bytes().into();
        let op = ContractOperation::new(Some(vk));
        state.operations = state.operations.insert(entry_point, op);
    }

    Ok(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use midnight_typed_state::{ContractMaintenanceAuthority, StateValue, StorageHashMap};

    fn make_counter_state(round: u64) -> ContractState<InMemoryDB> {
        ContractState::new(
            StateValue::Array(vec![StateValue::from(round)].into()),
            StorageHashMap::new(),
            ContractMaintenanceAuthority::default(),
        )
    }

    #[test]
    fn populate_verifier_keys_loads_increment() {
        let keys_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../devnet/contracts/counter/compiled");
        if !keys_dir.exists() {
            eprintln!("skipping: keys dir not found at {}", keys_dir.display());
            return;
        }

        let state = make_counter_state(0);
        assert!(state.operations.is_empty());

        let provider = crate::zk_config::FsZkConfigProvider::new(&keys_dir);
        let state = populate_verifier_keys(state, &provider, None).unwrap();

        let entry: midnight_onchain_runtime::state::EntryPointBuf = b"increment"[..].into();
        let op = state.operations.get(&entry).expect("increment operation");
        assert!(op.latest().is_some(), "verifier key should be present");
    }

    fn counter_compiled_dir() -> Option<std::path::PathBuf> {
        let d = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../devnet/contracts/counter/compiled");
        d.exists().then_some(d)
    }

    /// A mistyped `with_zk_config` path used to enumerate zero circuits and
    /// deploy a contract with an empty operations map: funds spent, every call
    /// rejected, recoverable only by a maintenance update.
    #[test]
    fn missing_keys_directory_is_an_error_not_an_empty_deploy() {
        let provider = crate::zk_config::FsZkConfigProvider::new("/nonexistent/compiledd");
        let err = populate_verifier_keys(make_counter_state(0), &provider, None)
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
        let Some(dir) = counter_compiled_dir() else {
            eprintln!("skipping: counter artifacts not built");
            return;
        };
        let provider = crate::zk_config::FsZkConfigProvider::new(&dir);
        let declared = vec![
            "increment".to_string(),
            "increment_by".to_string(),
            "decrement".to_string(),
        ];
        let err = populate_verifier_keys(make_counter_state(0), &provider, Some(&declared))
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
        let Some(dir) = counter_compiled_dir() else {
            eprintln!("skipping: counter artifacts not built");
            return;
        };
        let provider = crate::zk_config::FsZkConfigProvider::new(&dir);
        let declared = vec!["increment".to_string()];
        let err = populate_verifier_keys(make_counter_state(0), &provider, Some(&declared))
            .expect_err("an undeclared key file must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("increment_by"),
            "error should name the stale artifact, got: {msg}"
        );
    }

    #[test]
    fn declared_set_matching_the_directory_populates_every_circuit() {
        let Some(dir) = counter_compiled_dir() else {
            eprintln!("skipping: counter artifacts not built");
            return;
        };
        let provider = crate::zk_config::FsZkConfigProvider::new(&dir);
        let declared = vec!["increment".to_string(), "increment_by".to_string()];
        let state =
            populate_verifier_keys(make_counter_state(0), &provider, Some(&declared)).unwrap();
        for circuit in &declared {
            let entry: midnight_onchain_runtime::state::EntryPointBuf = circuit.as_bytes().into();
            assert!(
                state.operations.get(&entry).is_some(),
                "{circuit} should be registered"
            );
        }
    }

    #[test]
    fn deserialize_state_roundtrip() {
        let state = make_counter_state(42);
        let mut bytes = Vec::new();
        midnight_serialize::tagged_serialize(&state, &mut bytes).unwrap();
        let hex = hex::encode(&bytes);
        let restored = deserialize_state(&hex).unwrap();
        match restored.data.get_ref() {
            StateValue::Array(arr) => match arr.get(0).unwrap() {
                StateValue::Cell(sp) => {
                    let counter = u64::try_from(&*sp.value).unwrap();
                    assert_eq!(counter, 42);
                }
                _ => panic!("expected Cell"),
            },
            _ => panic!("expected Array"),
        }
    }
}
