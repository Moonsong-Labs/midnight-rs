//! A contract call can take an unshielded token from its caller.
//!
//! A circuit that calls `receiveUnshielded` leaves the call short of the token
//! at the guaranteed segment. The caller's wallet covers the deposit with its
//! own UTXOs, and takes back the change. A wallet that does not cover the
//! deposit refuses the call at the fill.
//!
//! The wallet gets the token from the contract first: the contract mints it to
//! itself and pays one UTXO to the caller. The deposit takes part of that UTXO.
//! The contract's balance must grow by the deposit, so a call that reached the
//! chain with no deposit fails. The wallet must keep the change: the ledger
//! refuses only a short transaction, so a cover with no change output lands
//! and loses it.
//!
//! Gated on a running devnet (`MIDNIGHT_NODE_URL`, `MIDNIGHT_INDEXER_URL`).

mod common;

use compact_bindgen::AlignedValue;
use midnight_contract::runtime::Value;
use midnight_contract::{Contract, ShieldedInputs};
use midnight_provider::MidnightProvider;
use midnight_types::UnshieldedTokenType;
use midnight_types::convert::IntoLedger;
use midnight_wallet::Wallet;

const DOMAIN_SEP: [u8; 32] = [0x3d; 32];
const MINTED: u128 = 1_000;
const PAID: u128 = 100;
const DEPOSITED: u128 = 40;

#[tokio::test]
async fn a_call_takes_an_unshielded_deposit_from_its_caller() {
    let Some((node_url, indexer_url)) = common::devnet_urls() else {
        return;
    };
    let keyed = common::compiled_dir("unshielded-payout");
    let info = common::load_compiled(&keyed);
    let program = common::program(&info);

    let provider = MidnightProvider::new(&node_url);
    let wallet = common::synced_wallet(&provider, &indexer_url, common::DEV_WALLET_SEED).await;
    let caller = wallet.unshielded_recipient().into_ledger();
    let provider = provider.with_wallet(wallet);

    let contract = Contract::deploy(&provider)
        .with_initial_state(common::contract_state(vec![]))
        .with_zk_config(&keyed)
        .send()
        .await
        .expect("submit deploy")
        .into_contract()
        .await
        .expect("deploy");
    eprintln!("deployed at {}", contract.address());

    let colour = midnight_contract::parse_contract_address(contract.address())
        .expect("contract address")
        .custom_unshielded_token_type(midnight_base_crypto::hash::HashOutput(DOMAIN_SEP));
    let colour_bytes = Value::AlignedValue(AlignedValue::from(colour.0.0));
    let call = |name: &'static str, args: Vec<(&'static str, Value)>| {
        let contract = &contract;
        let info = &info;
        let program = &program;
        async move {
            contract
                .send_call_with(
                    &common::exported_circuit(info, name).def,
                    program,
                    name,
                    &args,
                    &midnight_contract::runtime::NoWitnesses,
                    &[],
                    ShieldedInputs::default(),
                    Ok,
                )
                .await
                .unwrap_or_else(|e| panic!("{name} must reach the node: {e}"))
                .get_receipt()
                .await
                .unwrap_or_else(|e| panic!("{name} must apply: {e}"));
        }
    };

    call(
        "mintToSelf",
        vec![
            (
                "domainSep",
                Value::AlignedValue(AlignedValue::from(DOMAIN_SEP)),
            ),
            ("amount", Value::Integer(MINTED)),
        ],
    )
    .await;
    call(
        "payUser",
        vec![
            ("color", colour_bytes.clone()),
            ("amount", Value::Integer(PAID)),
            (
                "address",
                Value::AlignedValue(AlignedValue::from(caller.0.0)),
            ),
        ],
    )
    .await;
    let held = held_until(&provider, colour, |held| held == PAID).await;
    assert_eq!(
        held, PAID,
        "the caller must hold the payout before it deposits"
    );

    call(
        "deposit",
        vec![
            ("color", colour_bytes),
            ("amount", Value::Integer(DEPOSITED)),
        ],
    )
    .await;

    let state = midnight_contract::state::fetch_state(&provider, contract.address(), None)
        .await
        .expect("contract state");
    let balance = state
        .balance
        .get(&midnight_coin_structure::coin::TokenType::Unshielded(
            colour.into_ledger(),
        ))
        .map(|value| *value);
    assert_eq!(
        balance,
        Some(MINTED - PAID + DEPOSITED),
        "the contract must hold what it kept and what the caller deposited"
    );

    let change = PAID - DEPOSITED;
    let held = held_until(&provider, colour, |held| held == change).await;
    assert_eq!(held, change, "the caller must keep the change of its UTXO");
}

/// What the wallet of `provider` holds of `colour`, after a resync, once
/// `done` accepts it or after two minutes.
///
/// A payment shows up only when the indexer serves it, which can be after the
/// call returns, and no API waits for the indexer to reach a block (#164). So
/// this polls, and gives what it last saw for the caller's assertion to report.
async fn held_until(
    provider: &MidnightProvider<Wallet>,
    colour: UnshieldedTokenType,
    done: impl Fn(u128) -> bool,
) -> u128 {
    let mut seen = 0;
    for _ in 0..60 {
        provider.wallet().sync(provider).await.expect("resync");
        seen = provider.wallet().balance().await.unshielded_value(colour);
        if done(seen) {
            return seen;
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
    seen
}
