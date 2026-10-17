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
//!   - `RECEIVE_SHIELDED_DIR`: a compactc output dir
//!     (`compiler/analyzed-ir.sexp`, `keys/` and `zkir/`) with a circuit taking
//!     a single `ShieldedCoinInfo` argument that does `receiveShielded(coin)`
//!     (optionally followed by
//!     `sendImmediateShielded(coin, shieldedBurnAddress())`, i.e. the gateway
//!     `withdraw` shape).
//!   - `RECEIVE_SHIELDED_CIRCUIT` (optional, default `receive`): the circuit
//!     name.
//!
//! The full intents-swaps `withdraw` e2e (mint → burn → monitor observes →
//! release ceremony) lives in that repo, which has the gateway contract and the
//! ECDSA monitor; this harness proves the SDK half of the acceptance locally.

mod common;

use compact_bindgen::AlignedValue;
use midnight_contract::runtime::Value;
use midnight_contract::{
    BlockHash, CallOutcome, Contract, ContractError, ExtrinsicHash, ShieldedInputs,
    SpendableShieldedCoin,
};
use midnight_provider::MidnightProvider;

#[tokio::test]
async fn call_circuit_that_spends_the_callers_shielded_coin() {
    let Some((provider, dir)) = funded_provider().await else {
        return;
    };

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

    let value = coin.value;
    let (_, outcome) = deploy_and_receive(provider, &dir, coin, value)
        .await
        .expect("circuit call spending the caller's shielded coin");

    // A successful call hands back the identity of the transaction that carried
    // it, so a caller can log it or look it up. These used to be observable
    // only when the call failed.
    assert_ne!(
        outcome.extrinsic_hash,
        ExtrinsicHash::zero(),
        "a successful call must expose its extrinsic hash"
    );
    assert_ne!(
        outcome.block_hash,
        BlockHash::zero(),
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
    let Some((provider, dir)) = funded_provider().await else {
        return;
    };

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

    // The argument's value is what the contract receives, deliberately below
    // the attached coin's value.
    let token_type = coin.token_type;
    let (deployed, outcome) = deploy_and_receive(provider, &dir, coin, received)
        .await
        .expect("partially funded circuit call");
    assert_ne!(outcome.extrinsic_hash, ExtrinsicHash::zero());

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

/// A provider with the dev wallet attached, and the fixture directory that
/// `RECEIVE_SHIELDED_DIR` names. Returns `None` after a skip line when a
/// variable is missing.
async fn funded_provider() -> Option<(MidnightProvider, String)> {
    let (node_url, indexer_url) = common::devnet_urls()?;
    let Ok(dir) = std::env::var("RECEIVE_SHIELDED_DIR") else {
        eprintln!("skipping: needs RECEIVE_SHIELDED_DIR");
        return None;
    };
    let provider = MidnightProvider::new(&node_url, &indexer_url);
    let wallet = common::synced_wallet(&provider, common::DEV_WALLET_SEED).await;
    Some((provider.with_wallet(wallet), dir))
}

/// Deploy the fixture in `dir`, then call its receiving circuit with `coin`
/// attached as a shielded input and with `value` in the circuit's
/// `ShieldedCoinInfo` argument. The circuit is `RECEIVE_SHIELDED_CIRCUIT`, or
/// `receive` when that is unset.
async fn deploy_and_receive(
    provider: MidnightProvider,
    dir: &str,
    coin: SpendableShieldedCoin,
    value: u128,
) -> Result<(Contract<MidnightProvider>, CallOutcome<Option<Value>>), ContractError> {
    let circuit_name =
        std::env::var("RECEIVE_SHIELDED_CIRCUIT").unwrap_or_else(|_| "receive".to_string());
    let info = common::load_compiled(dir);
    let circuit = common::exported_circuit(&info, &circuit_name);
    let arg_name = circuit
        .arguments()
        .first()
        .map(|a| a.name.name().to_string())
        .expect("circuit must take a ShieldedCoinInfo argument");

    // A fixture that only receives/burns carries no user ledger state (an empty
    // array); deploy a fresh instance to call against.
    let deployed = Contract::deploy(provider)
        .with_initial_state(common::contract_state(vec![]))
        .with_zk_config(dir)
        .await
        .expect("deploy fixture");
    eprintln!("deployed fixture at {}", deployed.address());

    // The `ShieldedCoinInfo { nonce, color, value }` argument, built from the
    // exact coin the call spends.
    let coin_info = Value::AlignedValue(AlignedValue::concat(
        [
            AlignedValue::from(coin.nonce),
            AlignedValue::from(coin.token_type.0.0),
            AlignedValue::from(value),
        ]
        .iter(),
    ));

    // --- Gaps 2 + 3: attach the coin as a shielded input; the SDK spends that
    //     exact coin and, if the circuit forwards it, builds the transient. ---
    let outcome = deployed
        .call_with(
            &circuit.def,
            &common::program(&info),
            &circuit_name,
            &[(arg_name.as_str(), coin_info)],
            &midnight_contract::runtime::NoWitnesses,
            &[],
            ShieldedInputs { coins: vec![coin] },
        )
        .await?;
    Ok((deployed, outcome))
}
