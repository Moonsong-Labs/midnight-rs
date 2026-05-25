//! Reading and preparing on-chain contract state.
//!
//! Three flavours of state retrieval:
//!
//! - [`fetch_state`] / [`fetch_state_at`] go through the indexer.
//! - [`fetch_state_from_node`] goes through the node's `midnight_contractState`
//!   RPC. Use this when you want a hash-pinned view or when the indexer hasn't
//!   caught up to the block yet.
//!
//! [`with_zk_keys`] is the one *write* helper: it loads verifier keys from a
//! compiled contract directory and inserts them into a [`ContractState`]
//! before it goes on-chain.

use midnight_bindgen::{ContractState, InMemoryDB};
use midnight_onchain_runtime::state::{ContractOperation, EntryPointBuf};

use crate::error::ContractError;

/// Deserialize a hex-encoded contract state (as returned by the indexer or the
/// node RPC) into a [`ContractState`].
pub fn deserialize_state(hex_state: &str) -> Result<ContractState<InMemoryDB>, ContractError> {
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

/// Fetch contract state from a provider at a specific block offset. Pass
/// `None` to fetch the latest state.
pub async fn fetch_state_at<P: midnight_provider::Provider>(
    provider: &P,
    address: &str,
    offset: Option<midnight_provider::ContractActionOffset>,
) -> Result<ContractState<InMemoryDB>, ContractError> {
    let hex = provider
        .get_contract_state(address, offset)
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
    at_block_hash: Option<&str>,
) -> Result<ContractState<InMemoryDB>, ContractError> {
    let hex = provider
        .get_state_from_node(address, at_block_hash)
        .await
        .map_err(|e| ContractError::StateFetch(format!("node RPC: {e}")))?
        .ok_or_else(|| ContractError::NotFound(address.to_string()))?;
    deserialize_state(&hex)
}

/// Load verifier keys from a compiled contract directory and insert them into
/// the contract state's operations map.
///
/// Reads all `*.verifier` files from `{dir}/keys/`, deserializes each into a
/// `VerifierKey`, and inserts it keyed by the file stem (e.g.,
/// `keys/increment.verifier` → entry point `"increment"`).
///
/// Required for on-chain deployment — without verifier keys, the node cannot
/// verify ZK proofs for circuit calls.
pub fn with_zk_keys(
    mut state: ContractState<InMemoryDB>,
    keys_dir: impl AsRef<std::path::Path>,
) -> Result<ContractState<InMemoryDB>, ContractError> {
    use midnight_transient_crypto::proofs::VerifierKey;

    let base = keys_dir.as_ref();
    let keys_path = if base.join("keys").is_dir() {
        base.join("keys")
    } else {
        base.to_path_buf()
    };
    let entries = std::fs::read_dir(&keys_path).map_err(|e| {
        ContractError::Construction(format!(
            "cannot read keys directory {}: {e}",
            keys_path.display()
        ))
    })?;

    for entry in entries {
        let entry = entry.map_err(|e| ContractError::Construction(format!("read dir: {e}")))?;
        let path = entry.path();

        if path.extension().and_then(|e| e.to_str()) != Some("verifier") {
            continue;
        }

        let circuit_name = path.file_stem().and_then(|s| s.to_str()).ok_or_else(|| {
            ContractError::Construction(format!("invalid filename: {}", path.display()))
        })?;

        let bytes = std::fs::read(&path)
            .map_err(|e| ContractError::Construction(format!("read {}: {e}", path.display())))?;

        let vk: VerifierKey = midnight_serialize::tagged_deserialize(&mut bytes.as_slice())
            .map_err(|e| {
                ContractError::Construction(format!("deserialize {circuit_name}.verifier: {e}"))
            })?;

        let entry_point: EntryPointBuf = circuit_name.as_bytes().into();
        let op = ContractOperation::new(Some(vk));
        state.operations = state.operations.insert(entry_point, op);
    }

    Ok(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use midnight_bindgen::{ContractMaintenanceAuthority, StateValue, StorageHashMap};

    fn make_counter_state(round: u64) -> ContractState<InMemoryDB> {
        ContractState::new(
            StateValue::Array(vec![StateValue::from(round)].into()),
            StorageHashMap::new(),
            ContractMaintenanceAuthority::default(),
        )
    }

    #[test]
    fn with_zk_keys_loads_increment() {
        let keys_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/contracts/counter/compiled");
        if !keys_dir.exists() {
            eprintln!("skipping: keys dir not found at {}", keys_dir.display());
            return;
        }

        let state = make_counter_state(0);
        assert!(state.operations.is_empty());

        let state = with_zk_keys(state, &keys_dir).unwrap();

        let entry: midnight_onchain_runtime::state::EntryPointBuf = b"increment"[..].into();
        let op = state.operations.get(&entry).expect("increment operation");
        assert!(op.latest().is_some(), "verifier key should be present");
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
