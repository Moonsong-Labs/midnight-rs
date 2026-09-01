//! Live devnet E2E acceptance harness for issue #122: calling a circuit that
//! spends the caller's own shielded coin.
//!
//! Drives the full new surface end to end: enumerate the wallet's spendable
//! coins ([`MidnightProvider::spendable_shielded_coins`], gap 1), build the
//! circuit's `ShieldedCoinInfo` argument from the chosen coin, and attach that
//! same coin as a shielded input ([`ShieldedInputs`], gap 2) so a
//! `receiveShielded(coin)` (and, when the circuit also `sendImmediateShielded`s
//! it, the resulting contract-owned transient, gap 3) balances.
//!
//! Gated on a running devnet + indexer AND a compiled fixture whose circuit
//! spends the caller's coin:
//!   - `MIDNIGHT_NODE_URL`, `MIDNIGHT_INDEXER_URL`: the devnet + indexer.
//!   - `RECEIVE_SHIELDED_DIR`: a compiled-contract dir (`analyzed-ir.sexp` +
//!     `compiled/`) with a circuit taking a single `ShieldedCoinInfo` argument
//!     that does `receiveShielded(coin)` (optionally followed by
//!     `sendImmediateShielded(coin, shieldedBurnAddress())`, i.e. the gateway
//!     `withdraw` shape).
//!   - `RECEIVE_SHIELDED_CIRCUIT` (optional, default `receive`): the circuit
//!     name.
//!
//! The full intents-swaps `withdraw` e2e (mint → burn → monitor observes →
//! release ceremony) lives in that repo, which has the gateway contract and the
//! ECDSA monitor; this harness proves the SDK half of the acceptance locally.

use compact_bindgen::AlignedValue;
use midnight_contract::runtime::Value;
use midnight_contract::{Contract, ShieldedInputs};
use midnight_wallet::{LocalWallet, Wallet};

#[tokio::test]
async fn call_circuit_that_spends_the_callers_shielded_coin() {
    let (node_url, indexer_url, dir) = match (
        std::env::var("MIDNIGHT_NODE_URL").ok(),
        std::env::var("MIDNIGHT_INDEXER_URL").ok(),
        std::env::var("RECEIVE_SHIELDED_DIR").ok(),
    ) {
        (Some(n), Some(i), Some(d)) => (n, i, d),
        _ => {
            eprintln!(
                "skipping: needs MIDNIGHT_NODE_URL + MIDNIGHT_INDEXER_URL + RECEIVE_SHIELDED_DIR"
            );
            return;
        }
    };
    let circuit_name =
        std::env::var("RECEIVE_SHIELDED_CIRCUIT").unwrap_or_else(|_| "receive".to_string());

    // --- Load the circuit IR + defs from the compiled fixture ---
    let info_json =
        std::fs::read_to_string(format!("{dir}/analyzed-ir.sexp")).expect("read contract-info");
    let info: compact_codegen::types::ContractInfo =
        compact_codegen::artifact::load_str(&info_json).expect("parse contract-info");
    let circuit = info
        .circuits
        .iter()
        .find(|c| c.name == circuit_name)
        .unwrap_or_else(|| panic!("circuit `{circuit_name}` not found in fixture"));
    let ir = &circuit.def;
    let program =
        midnight_contract::interpreter::Program::new(&info.helpers, &info.witnesses, &info.natives);

    let arg_name = circuit
        .arguments()
        .first()
        .map(|a| a.name.name().to_string())
        .expect("circuit must take a ShieldedCoinInfo argument");

    // --- Funder wallet syncs and deploys the fixture ---
    let funder_seed = midnight_provider::WalletSeed::try_from_hex_str(
        "0000000000000000000000000000000000000000000000000000000000000001",
    )
    .unwrap();
    let provider =
        midnight_provider::MidnightProvider::new(&node_url, &indexer_url).expect("provider");
    let wallet = Wallet::sync(
        provider.indexer_url(),
        funder_seed,
        midnight_provider::Network::Undeployed,
    )
    .await
    .expect("funder sync");
    let provider = provider.with_wallet(LocalWallet::new(wallet));

    // --- Gap 1: address a specific spendable coin (with its nonce) ---
    let coins = provider
        .spendable_shielded_coins()
        .await
        .expect("spendable coins");
    let coin = match coins.into_iter().next() {
        Some(c) => c,
        None => {
            eprintln!(
                "skipping: funder wallet has no spendable shielded coin to burn \
                 (fund it with a shielded coin of the fixture's expected token type first)"
            );
            return;
        }
    };
    eprintln!(
        "burning coin: token {}… value {}",
        &coin.token_type_hex()[..8],
        coin.value
    );

    // A fixture that only receives/burns carries no user ledger state (an empty
    // array); deploy a fresh instance to call against. Move the provider into
    // the handle (we've already read the spendable coins above).
    let deployed = Contract::deploy(provider)
        .with_initial_state(midnight_contract::InitialState::default())
        .with_zk_config(&dir)
        .await
        .expect("deploy fixture");
    eprintln!("deployed fixture at {}", deployed.address());

    // Build the `ShieldedCoinInfo { nonce, color, value }` argument from the
    // exact coin we are about to spend.
    let coin_info = Value::AlignedValue(AlignedValue::concat(
        [
            AlignedValue::from(coin.nonce),
            AlignedValue::from(coin.token_type.0.0),
            AlignedValue::from(coin.value),
        ]
        .iter(),
    ));

    // --- Gaps 2 + 3: attach the coin as a shielded input; the SDK spends that
    //     exact coin and, if the circuit forwards it, builds the transient. ---
    let outcome = deployed
        .call_with(
            ir,
            &program,
            &circuit_name,
            &[(arg_name.as_str(), coin_info)],
            &midnight_contract::runtime::NoWitnesses,
            &[],
            ShieldedInputs { coins: vec![coin] },
        )
        .await
        .expect("circuit call spending the caller's shielded coin");

    // A successful call hands back the identity of the transaction that carried
    // it, so a caller can log it or look it up. These used to be observable
    // only when the call failed.
    assert_ne!(
        outcome.extrinsic_hash, [0u8; 32],
        "a successful call must expose its extrinsic hash"
    );
    assert_ne!(
        outcome.block_hash, [0u8; 32],
        "a successful call must expose the block it landed in"
    );
    eprintln!(
        "call spending the caller's shielded coin succeeded ✓ (tx {}, block {})",
        hex::encode(outcome.extrinsic_hash),
        hex::encode(outcome.block_hash)
    );
}

/// Funding a receive from a coin worth more than the circuit takes must return
/// the remainder. Zswap asserts only `balance >= 0` per (token, segment), so an
/// unclaimed surplus is accepted by the chain and destroyed; the change output
/// is what makes the delta exactly zero and keeps the difference spendable.
///
/// Same gating as the test above. Uses one coin and a circuit argument worth
/// half of it, so the wallet must come out holding a fresh coin for the other
/// half.
#[tokio::test]
async fn attaching_more_than_the_circuit_receives_returns_change() {
    let (node_url, indexer_url, dir) = match (
        std::env::var("MIDNIGHT_NODE_URL").ok(),
        std::env::var("MIDNIGHT_INDEXER_URL").ok(),
        std::env::var("RECEIVE_SHIELDED_DIR").ok(),
    ) {
        (Some(n), Some(i), Some(d)) => (n, i, d),
        _ => {
            eprintln!(
                "skipping: needs MIDNIGHT_NODE_URL + MIDNIGHT_INDEXER_URL + RECEIVE_SHIELDED_DIR"
            );
            return;
        }
    };
    let circuit_name =
        std::env::var("RECEIVE_SHIELDED_CIRCUIT").unwrap_or_else(|_| "receive".to_string());

    let info_json =
        std::fs::read_to_string(format!("{dir}/analyzed-ir.sexp")).expect("read contract-info");
    let info: compact_codegen::types::ContractInfo =
        compact_codegen::artifact::load_str(&info_json).expect("parse contract-info");
    let circuit = info
        .circuits
        .iter()
        .find(|c| c.name == circuit_name)
        .unwrap_or_else(|| panic!("circuit `{circuit_name}` not found in fixture"));
    let ir = &circuit.def;
    let program =
        midnight_contract::interpreter::Program::new(&info.helpers, &info.witnesses, &info.natives);

    let arg_name = circuit
        .arguments()
        .first()
        .map(|a| a.name.name().to_string())
        .expect("circuit must take a ShieldedCoinInfo argument");

    let funder_seed = midnight_provider::WalletSeed::try_from_hex_str(
        "0000000000000000000000000000000000000000000000000000000000000001",
    )
    .unwrap();
    let provider =
        midnight_provider::MidnightProvider::new(&node_url, &indexer_url).expect("provider");
    let wallet = Wallet::sync(
        provider.indexer_url(),
        funder_seed,
        midnight_provider::Network::Undeployed,
    )
    .await
    .expect("funder sync");
    let provider = provider.with_wallet(LocalWallet::new(wallet));

    let coins = provider
        .spendable_shielded_coins()
        .await
        .expect("spendable coins");
    // Needs a coin big enough to split into a received half and a change half.
    let coin = match coins.into_iter().find(|c| c.value >= 2) {
        Some(c) => c,
        None => {
            eprintln!("skipping: funder wallet has no shielded coin worth at least 2");
            return;
        }
    };
    let received = coin.value / 2;
    let expected_change = coin.value - received;
    eprintln!(
        "attaching coin worth {}, circuit receives {received}, expecting {expected_change} back",
        coin.value
    );

    let deployed = Contract::deploy(provider)
        .with_initial_state(midnight_contract::InitialState::default())
        .with_zk_config(&dir)
        .await
        .expect("deploy fixture");

    // The argument's value is what the contract receives, deliberately below
    // the attached coin's value. The nonce is fresh: the received coin is a new
    // coin the caller funds, not a re-commitment of the one being spent.
    let coin_info = Value::AlignedValue(AlignedValue::concat(
        [
            AlignedValue::from(coin.nonce),
            AlignedValue::from(coin.token_type.0.0),
            AlignedValue::from(received),
        ]
        .iter(),
    ));

    let token_type = coin.token_type;
    let outcome = deployed
        .call_with(
            ir,
            &program,
            &circuit_name,
            &[(arg_name.as_str(), coin_info)],
            &midnight_contract::runtime::NoWitnesses,
            &[],
            ShieldedInputs { coins: vec![coin] },
        )
        .await
        .expect("partially funded circuit call");
    assert_ne!(outcome.extrinsic_hash, [0u8; 32]);

    // The change is a fresh coin, so it only shows up once the indexer replays
    // the transaction's zswap events.
    let provider = deployed.provider();
    let mut found = false;
    for _ in 0..20 {
        provider.resync_wallet().await.expect("resync");
        if provider
            .spendable_shielded_coins()
            .await
            .expect("spendable coins")
            .iter()
            .any(|c| c.token_type == token_type && c.value == expected_change)
        {
            found = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    }
    assert!(
        found,
        "the wallet must hold a coin worth {expected_change} after funding a \
         receive of {received} from a larger coin; without a change output the \
         chain accepts the surplus and destroys it"
    );
    eprintln!("change of {expected_change} returned to the wallet ✓");
}
