//! Helpers for the test binaries of this crate. Each binary includes this
//! module with `mod common;`.

#![allow(dead_code, reason = "each test binary uses a subset of the helpers")]

use std::path::Path;
use std::time::Duration;

use compact_bindgen::{
    AlignedValue, ContractMaintenanceAuthority, ContractState, InMemoryDB, StateValue,
    StorageHashMap,
};
use compact_codegen::types::{Circuit, ContractInfo};
use midnight_base_crypto::hash::PersistentHashWriter;
use midnight_base_crypto::repr::BinaryHashRepr;
use midnight_base_crypto::time::Timestamp;
use midnight_contract::interpreter;
use midnight_contract::runtime::WitnessProvider;
use midnight_contract::{ShieldedTokenType, SpendableShieldedCoin};
use midnight_provider::{MidnightProvider, Network};
use midnight_transient_crypto::fab::ValueReprAlignedValue;
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

/// Resync the wallet of `provider` until it holds a coin of `token_type`
/// worth `value`, and return that coin.
///
/// A coin shows up only when the indexer serves its zswap event, which can be
/// after the call that pays it returns. No API waits for the indexer to reach
/// a given block (#164).
///
/// # Panics
///
/// When no such coin shows up after the last resync. The message names the
/// coin as `what`.
pub(crate) async fn resync_until_coin(
    provider: &MidnightProvider,
    token_type: ShieldedTokenType,
    value: u128,
    what: &str,
) -> SpendableShieldedCoin {
    let mut coins = Vec::new();
    for _ in 0..60 {
        provider.resync_wallet().await.expect("resync");
        coins = provider
            .spendable_shielded_coins()
            .await
            .expect("spendable coins");
        if let Some(index) = coins
            .iter()
            .position(|c| c.token_type == token_type && c.value == value)
        {
            return coins.swap_remove(index);
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    panic!(
        "the wallet does not hold {what} (token {token_type}, value {value}); \
         shielded coins seen: {:?}",
        coins
            .iter()
            .map(|c| (c.token_type_hex(), c.value))
            .collect::<Vec<_>>()
    );
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

/// No witnesses, a scratch private state, the zero address, the epoch.
pub(crate) fn epoch() -> interpreter::Env<'static> {
    interpreter::Env::new(Timestamp::from_secs(0))
}

/// `epoch()` with `witnesses` answering the witness calls.
pub(crate) fn with_witnesses(witnesses: &dyn WitnessProvider) -> interpreter::Env<'_> {
    interpreter::Env {
        witnesses,
        ..epoch()
    }
}

/// The `persistentHash` of `value`, computed with base-crypto and not with the
/// interpreter's builtin, so that a test can check the interpreter against it.
pub(crate) fn persistent_hash(value: AlignedValue) -> AlignedValue {
    let mut hasher = PersistentHashWriter::default();
    ValueReprAlignedValue(value).binary_repr(&mut hasher);
    AlignedValue::from(hasher.finalize().0)
}
