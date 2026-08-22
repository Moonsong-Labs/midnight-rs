//! A contract can pay an unshielded token to a user address.
//!
//! The contract mints an unshielded token to itself, then sends some of it to a
//! user. The send claims an unshielded spend in the call's transcript, and
//! verification requires a real unshielded offer to cover every claimed one, so
//! the call builds one from the transcript. Without it the node refuses the
//! transaction with `EffectsCheckFailure`.
//!
//! The recipient is a second wallet, and the assertion is that its balance
//! grew. Paying the caller's own address would pass even if every payout were
//! wrongly owned by the caller, which is the distinction `PayoutOutput` exists
//! for; asserting only that the call returned would pass on a transaction that
//! paid nobody.
//!
//! Gated on a running devnet (`MIDNIGHT_NODE_URL`, `MIDNIGHT_INDEXER_URL`) and
//! on the contract beside it having been compiled, which `make
//! compile-contracts` does. The keys are generated rather than committed, so
//! this skips wherever compactc has not run. Override the directory with
//! `PAYOUT_KEYED_DIR`.

use compact_bindgen::{
    AlignedValue, ContractMaintenanceAuthority, ContractState, StateValue, StorageHashMap,
};
use midnight_coin_structure::coin::UnshieldedTokenType;
use midnight_contract::Contract;
use midnight_contract::runtime::Value;
use midnight_provider::MidnightProvider;

const DOMAIN_SEP: [u8; 32] = [0x33; 32];
const MINTED: u128 = 1_000;
const PAID: u128 = 10;

#[tokio::test]
async fn a_contract_pays_an_unshielded_token_to_a_user() {
    let (Ok(node_url), Ok(indexer_url)) = (
        std::env::var("MIDNIGHT_NODE_URL"),
        std::env::var("MIDNIGHT_INDEXER_URL"),
    ) else {
        eprintln!("skipping: needs MIDNIGHT_NODE_URL + MIDNIGHT_INDEXER_URL");
        return;
    };
    let keyed = std::env::var("PAYOUT_KEYED_DIR").unwrap_or_else(|_| {
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../devnet/contracts/unshielded-payout/compiled"
        )
        .to_string()
    });
    if !std::path::Path::new(&format!("{keyed}/analyzed-ir.sexp")).exists() {
        eprintln!("skipping: {keyed} is empty; run `make compile-contracts` first");
        return;
    }

    let info_json =
        std::fs::read_to_string(format!("{keyed}/analyzed-ir.sexp")).expect("read contract-info");
    let info: compact_codegen::types::ContractInfo =
        compact_codegen::artifact::load_str(&info_json).expect("parse contract-info");
    let program =
        midnight_contract::interpreter::Program::new(&info.helpers, &info.witnesses, &info.natives);
    let circuit = |name: &str| {
        info.circuits
            .iter()
            .find(|c| c.name == name)
            .unwrap_or_else(|| panic!("circuit {name}"))
    };

    let seed = midnight_provider::WalletSeed::try_from_hex_str(
        "0000000000000000000000000000000000000000000000000000000000000001",
    )
    .unwrap();
    let provider = MidnightProvider::new(&node_url, &indexer_url)
        .expect("provider")
        .sync_wallet(seed.clone(), midnight_provider::Network::Undeployed)
        .await
        .expect("sync");

    let contract = Contract::deploy(provider)
        .with_initial_state(ContractState::new(
            StateValue::Array(vec![].into()),
            StorageHashMap::new(),
            ContractMaintenanceAuthority::default(),
        ))
        .with_zk_config(&keyed)
        .await
        .expect("deploy");
    eprintln!("deployed at {}", contract.address());

    // The minted token is derived from the contract's own address, so it can
    // only be named once the contract exists.
    let colour = midnight_contract::parse_address(contract.address())
        .expect("contract address")
        .custom_unshielded_token_type(midnight_base_crypto::hash::HashOutput(DOMAIN_SEP));

    // Mint to the contract itself, so it has a balance to pay out of.
    contract
        .call_with(
            &circuit("mintToSelf").def,
            &program,
            "mintToSelf",
            &[
                (
                    "domainSep",
                    Value::AlignedValue(AlignedValue::from(DOMAIN_SEP)),
                ),
                ("amount", Value::Integer(MINTED)),
            ],
            &midnight_contract::runtime::NoWitnesses,
            &[],
            midnight_contract::ShieldedInputs::default(),
        )
        .await
        .expect("the contract must be able to mint to itself");

    // A different wallet from the one that deploys and pays the fees, so an
    // output wrongly owned by the caller would fail this.
    let recipient_seed = midnight_provider::WalletSeed::try_from_hex_str(
        "0000000000000000000000000000000000000000000000000000000000000002",
    )
    .unwrap();
    let recipient =
        midnight_helpers::UnshieldedWallet::default(recipient_seed.clone()).user_address;
    let recipient_provider = MidnightProvider::new(&node_url, &indexer_url)
        .expect("provider")
        .sync_wallet(recipient_seed, midnight_provider::Network::Undeployed)
        .await
        .expect("recipient sync");

    let before = held(&recipient_provider, colour).await;
    eprintln!("recipient holds {before} before the payout");

    contract
        .call_with(
            &circuit("payUser").def,
            &program,
            "payUser",
            &[
                ("color", Value::AlignedValue(AlignedValue::from(colour.0.0))),
                ("amount", Value::Integer(PAID)),
                (
                    "address",
                    Value::AlignedValue(AlignedValue::from(recipient.0.0)),
                ),
            ],
            &midnight_contract::runtime::NoWitnesses,
            &[],
            midnight_contract::ShieldedInputs::default(),
        )
        .await
        .expect("paying a user an unshielded token must be accepted");

    // The call returns once the transaction is in a block, which is before the
    // indexer has served it, so poll rather than read once.
    let after = held_until(&recipient_provider, colour, before + PAID).await;
    eprintln!("recipient holds {after} after the payout");
    assert_eq!(
        after,
        before + PAID,
        "the payout must reach the recipient, not merely be accepted"
    );
}

/// What the wallet holds of `colour` right now.
async fn held(provider: &MidnightProvider, colour: UnshieldedTokenType) -> u128 {
    provider.resync_wallet().await.expect("resync");
    provider
        .balance()
        .await
        .expect("balance")
        .unshielded
        .iter()
        .filter(|u| u.token_type == colour)
        .map(|u| u.value)
        .sum()
}

/// [`held`], retried until it reaches `target` or two minutes elapse.
///
/// There is no way to wait for the indexer to reach a given block (#164), so
/// this polls. The recipient is a second wallet, whose sync adds to the wait.
///
/// Returns whatever it last saw, so a caller's assertion reports the real
/// figure rather than a timeout.
async fn held_until(
    provider: &MidnightProvider,
    colour: UnshieldedTokenType,
    target: u128,
) -> u128 {
    let mut seen = 0;
    for _ in 0..60 {
        seen = held(provider, colour).await;
        if seen >= target {
            return seen;
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
    seen
}
