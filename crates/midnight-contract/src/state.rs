//! Reading and preparing on-chain contract state.
//!
//! Bindgen-generated code reads a contract's state with [`fetch_state`],
//! through a [`MidnightProvider`].
//!
//! The other helpers in this module are `pub(crate)` plumbing for the deploy,
//! call and maintenance paths.

use std::sync::Arc;

use midnight_base_crypto::time::Timestamp;
use midnight_onchain_runtime::state::EntryPointBuf;
use midnight_provider::MidnightProvider;
use midnight_typed_state::{ContractState, InMemoryDB, tagged_deserialize};
use midnight_types::BlockHash;

use crate::error::{ContractError, KeyMismatch};

/// Fetch a contract's state through a provider and deserialize it.
///
/// The read is at `at_block_hash` when given, else at the node's best block.
/// Returns [`ContractError::NotFound`] when no contract exists at `address`,
/// and [`ContractError::StateFetch`] when the state does not decode.
pub async fn fetch_state<W>(
    provider: &MidnightProvider<W>,
    address: &str,
    at_block_hash: Option<BlockHash>,
) -> Result<ContractState<InMemoryDB>, ContractError> {
    let hex = provider
        .get_contract_state(address, at_block_hash)
        .await?
        .ok_or_else(|| ContractError::NotFound(address.to_string()))?;
    let bytes =
        hex::decode(&hex).map_err(|e| ContractError::StateFetch(format!("hex decode: {e}")))?;
    tagged_deserialize(bytes.as_slice())
        .map_err(|e| ContractError::StateFetch(format!("deserialize: {e}")))
}

/// A contract's state and the time of the block it was read at: what a call
/// runs its circuit against.
pub(crate) struct StateAtBlock {
    pub(crate) contract_state: ContractState<InMemoryDB>,
    /// The block's time in whole seconds, the unit of the chain's clock
    /// checks.
    pub(crate) time: Timestamp,
    /// The hash of the block.
    pub(crate) hash: BlockHash,
}

/// Read a contract's state and the block time at one block: `at_block` when
/// given, else the node's best block.
///
/// One hash serves both reads, so the state and the time never come from
/// two blocks.
pub(crate) async fn state_at_block<W>(
    provider: &MidnightProvider<W>,
    address: &str,
    at_block: Option<BlockHash>,
) -> Result<StateAtBlock, ContractError> {
    let hash = match at_block {
        Some(hash) => hash,
        None => provider.best_block_hash().await?,
    };
    let contract_state = fetch_state(provider, address, Some(hash)).await?;
    let time = provider.get_block_timestamp(hash).await?;
    Ok(StateAtBlock {
        contract_state,
        time: Timestamp::from_secs(time.as_secs()),
        hash,
    })
}

/// Load verifier keys from a [`ZkConfigProvider`] and insert them into the
/// contract state's operations map, keyed by circuit id. See
/// [`verifier_keys`].
///
/// [`ZkConfigProvider`]: crate::zk_config::ZkConfigProvider
pub(crate) async fn populate_verifier_keys(
    mut state: ContractState<InMemoryDB>,
    zk_config: &Arc<dyn crate::zk_config::ZkConfigProvider>,
    declared: Option<Vec<String>>,
) -> Result<ContractState<InMemoryDB>, ContractError> {
    let keys = crate::zk_config::read_blocking(zk_config, move |zk_config| {
        verifier_keys(zk_config, declared.as_deref())
    })
    .await?;
    for (circuit, key) in keys {
        let op = crate::ledger::state::operation(&circuit, &key)?;
        state.operations = state.operations.insert(circuit.as_bytes().into(), op);
    }
    Ok(state)
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
            Ok((circuit, bytes))
        })
        .collect()
}

/// Check that `state` holds `verifier_key`, the bytes of `circuit`'s compiled
/// `*.verifier` artifact, as a key of the circuit's operation.
///
/// An operation can hold a key of each version. A proof verifies against the
/// key of its own version, and the tag of the bytes names that version, so
/// the local key passes when it equals either key.
pub(crate) fn check_verifier_key(
    state: &ContractState<InMemoryDB>,
    circuit: &str,
    verifier_key: &[u8],
) -> Result<(), ContractError> {
    let mismatch = |reason| ContractError::VerifierKeyMismatch {
        circuit: circuit.to_owned(),
        reason,
    };
    let entry_point: EntryPointBuf = circuit.as_bytes().into();
    let op = state
        .operations
        .get(&entry_point)
        .ok_or_else(|| mismatch(KeyMismatch::Missing))?;
    if op.v2.is_none() && op.v3.is_none() {
        return Err(mismatch(KeyMismatch::Keyless));
    }
    let what = format!("verifier key {circuit}");
    let v2 = op
        .v2
        .as_ref()
        .map(|key| crate::call::tagged_bytes(key, &what))
        .transpose()?;
    let v3 = op
        .v3
        .as_ref()
        .map(|key| crate::call::tagged_bytes(key, &what))
        .transpose()?;
    if ![v2, v3].iter().flatten().any(|key| key == verifier_key) {
        return Err(mismatch(KeyMismatch::Mismatched));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counter_compiled_dir() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../devnet/contracts/counter/compiled")
    }

    fn verifier_key_file(contract: &str, circuit: &str) -> Vec<u8> {
        std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
                "../../devnet/contracts/{contract}/compiled/keys/{circuit}.verifier"
            )),
        )
        .unwrap()
    }

    /// A `verifier-key[v6]`. The compiler writes only `verifier-key[v7]`.
    fn v6_key() -> Vec<u8> {
        let mut bytes = Vec::new();
        midnight_serialize::tagged_serialize(
            &*midnight_helpers::mn_ledger::dust::SPEND_VK,
            &mut bytes,
        )
        .unwrap();
        bytes
    }

    fn mismatch(result: Result<(), ContractError>) -> KeyMismatch {
        match result {
            Err(ContractError::VerifierKeyMismatch { reason, .. }) => reason,
            other => panic!("expected a verifier key mismatch, got {other:?}"),
        }
    }

    /// The deploy writes each key in the encoding that the chain stores. A
    /// check that compares another encoding refuses every call of a deployed
    /// contract.
    #[tokio::test]
    async fn the_check_passes_only_the_key_that_the_contract_holds() {
        let zk_config: Arc<dyn crate::zk_config::ZkConfigProvider> = Arc::new(
            crate::zk_config::FsZkConfigProvider::new(counter_compiled_dir()),
        );
        let mut state = populate_verifier_keys(ContractState::default(), &zk_config, None)
            .await
            .unwrap();
        let increment = verifier_key_file("counter", "increment");
        let increment_by = verifier_key_file("counter", "increment_by");

        check_verifier_key(&state, "increment", &increment).unwrap();
        assert_eq!(
            mismatch(check_verifier_key(&state, "increment", &increment_by)),
            KeyMismatch::Mismatched
        );
        assert_eq!(
            mismatch(check_verifier_key(&state, "decrement", &increment)),
            KeyMismatch::Missing
        );

        state.operations = state.operations.insert(
            b"increment"[..].into(),
            midnight_onchain_runtime::state::ContractOperation::new(None, None),
        );
        assert_eq!(
            mismatch(check_verifier_key(&state, "increment", &increment)),
            KeyMismatch::Keyless
        );
    }

    /// A maintenance update can add a key of the other version beside the
    /// first one. A check that reads one of the two slots only refuses the key
    /// in the other.
    #[test]
    fn a_key_of_either_version_passes_on_an_operation_that_holds_both() {
        let v6 = v6_key();
        let v7 = verifier_key_file("counter", "increment");
        let mut op = crate::ledger::state::operation("increment", &v6).unwrap();
        op.v3 = crate::ledger::state::operation("increment", &v7)
            .unwrap()
            .v3;
        let mut state = ContractState::<InMemoryDB>::default();
        state.operations = state.operations.insert(b"increment"[..].into(), op);

        for key in [v6, v7] {
            check_verifier_key(&state, "increment", &key).unwrap();
        }
    }

    #[test]
    fn verifier_keys_loads_increment() {
        let provider = crate::zk_config::FsZkConfigProvider::new(counter_compiled_dir());
        let keys = verifier_keys(&provider, None).unwrap();

        let (_, key) = keys
            .iter()
            .find(|(circuit, _)| circuit == "increment")
            .expect("increment verifier key");
        assert!(key.starts_with(b"midnight:verifier-key[v7]:"));
    }

    /// The upstream constructors panic on a key of any other tag, and this
    /// check is what keeps such an artifact away from them.
    #[test]
    fn a_verifier_key_of_an_unknown_tag_is_refused() {
        let key = b"midnight:verifier-key[v5]:\x00\x01";
        match crate::ledger::state::operation("increment", key) {
            Err(ContractError::Construction(message)) => {
                assert!(message.contains("verifier-key[v5]"), "{message}");
            }
            other => panic!("expected a Construction error, got {other:?}"),
        }
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
