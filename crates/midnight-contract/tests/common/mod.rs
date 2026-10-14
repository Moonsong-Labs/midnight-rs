//! Helpers for the test binaries of this crate. Each binary includes this
//! module with `mod common;`.

#![allow(dead_code, reason = "each test binary uses a subset of the helpers")]

use std::path::Path;

use compact_bindgen::{
    ContractMaintenanceAuthority, ContractState, InMemoryDB, StateValue, StorageHashMap,
};
use compact_codegen::types::{Circuit, ContractInfo};
use midnight_contract::interpreter;
use midnight_provider::{MidnightProvider, Network};
use midnight_wallet::Wallet;

/// The seed of the wallet that the dev devnet funds at genesis.
pub(crate) const DEV_WALLET_SEED: &str =
    "0000000000000000000000000000000000000000000000000000000000000001";

/// A second seed, for a wallet that a test pays to.
pub(crate) const RECIPIENT_SEED: &str =
    "0000000000000000000000000000000000000000000000000000000000000002";

/// The devnet's node and indexer URLs, from `MIDNIGHT_NODE_URL` and
/// `MIDNIGHT_INDEXER_URL`.
///
/// Returns `None` after a skip line when either variable is missing.
///
/// # Panics
///
/// When either variable is missing while `MIDNIGHT_E2E` is set. The make
/// targets that run the devnet tests set it, so a test that they run never
/// skips.
pub(crate) fn devnet_urls() -> Option<(String, String)> {
    match (
        std::env::var("MIDNIGHT_NODE_URL"),
        std::env::var("MIDNIGHT_INDEXER_URL"),
    ) {
        (Ok(node_url), Ok(indexer_url)) => Some((node_url, indexer_url)),
        _ if std::env::var_os("MIDNIGHT_E2E").is_some() => {
            panic!("MIDNIGHT_NODE_URL or MIDNIGHT_INDEXER_URL is missing while MIDNIGHT_E2E is set")
        }
        _ => {
            eprintln!("skipping: needs MIDNIGHT_NODE_URL + MIDNIGHT_INDEXER_URL");
            None
        }
    }
}

/// The wallet of `seed` on the undeployed network, synced through `provider`.
///
/// # Panics
///
/// When `seed` is not a 32-byte hex seed, or when the sync fails.
pub(crate) async fn synced_wallet(provider: &MidnightProvider, seed: &str) -> Wallet {
    Wallet::from_seed(seed)
        .unwrap()
        .with_network(Network::Undeployed)
        .sync(provider)
        .await
        .unwrap_or_else(|e| panic!("sync of seed {seed}: {e:?}"))
}

/// The compactc output directory of the devnet contract `contract`, which
/// holds `compiler/`, `keys/` and `zkir/`.
pub(crate) fn compiled_dir(contract: &str) -> String {
    format!(
        "{}/../../devnet/contracts/{contract}/compiled",
        env!("CARGO_MANIFEST_DIR")
    )
}

/// A contract state whose ledger is the array `cells`, with no operations and
/// the default maintenance authority.
pub(crate) fn contract_state(cells: Vec<StateValue<InMemoryDB>>) -> ContractState<InMemoryDB> {
    ContractState::new(
        StateValue::Array(cells.into()),
        StorageHashMap::new(),
        ContractMaintenanceAuthority::default(),
    )
}

/// An overriding directory from the environment. An empty value reads as
/// unset, so `VAR=` in a make recipe does not resolve to the process's
/// working directory.
pub(crate) fn env_dir(var: &str) -> Option<String> {
    std::env::var(var).ok().filter(|v| !v.is_empty())
}

/// The contract info in the compactc output directory `dir`, read from
/// `compiler/analyzed-ir.sexp`.
///
/// # Panics
///
/// When the file cannot be read or does not parse.
pub(crate) fn load_compiled(dir: &str) -> ContractInfo {
    let path = Path::new(dir).join("compiler/analyzed-ir.sexp");
    compact_codegen::artifact::load(&path)
        .unwrap_or_else(|e| panic!("load {}: {e}", path.display()))
}

/// The circuit that `info` exports under `name`.
///
/// # Panics
///
/// When `info` exports no circuit of that name.
pub(crate) fn exported_circuit<'a>(info: &'a ContractInfo, name: &str) -> &'a Circuit {
    info.circuits
        .iter()
        .find(|c| c.name == name)
        .unwrap_or_else(|| panic!("circuit {name} not found"))
}

/// The tables a circuit body resolves a `call` through: every circuit the
/// contract defines, plus its witness and native declarations.
pub(crate) fn program(info: &ContractInfo) -> interpreter::Program<'_> {
    interpreter::Program::new(&info.helpers, &info.witnesses, &info.natives)
}
