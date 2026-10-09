//! `.simulate()` runs a call on the contract's state at one block and returns
//! the circuit's value and the ledger after the call. It proves nothing,
//! pays no fee and submits nothing, so it needs no wallet.
//!
//! Gated on a running devnet (`MIDNIGHT_NODE_URL`, `MIDNIGHT_INDEXER_URL`).
//! Under `make test-e2e`, which sets `MIDNIGHT_E2E`, a missing URL panics.

mod counter {
    compact_bindgen::contract!("../../devnet/contracts/counter/compiled/compiler/analyzed-ir.sexp");
}

use midnight_contract::NodeBlockHash;
use midnight_provider::{MidnightProvider, Network, WalletSeed};
use midnight_wallet::{LocalWallet, Wallet};

const ZK_KEYS_DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../devnet/contracts/counter/compiled"
);
const DEV_WALLET_SEED: &str = "0000000000000000000000000000000000000000000000000000000000000001";

#[tokio::test]
async fn a_simulated_call_returns_the_value_and_the_next_ledger_with_no_wallet() {
    let (Ok(node_url), Ok(indexer_url)) = (
        std::env::var("MIDNIGHT_NODE_URL"),
        std::env::var("MIDNIGHT_INDEXER_URL"),
    ) else {
        if std::env::var_os("MIDNIGHT_E2E").is_some() {
            panic!("MIDNIGHT_NODE_URL or MIDNIGHT_INDEXER_URL is missing under make test-e2e");
        }
        eprintln!("skipping: needs MIDNIGHT_NODE_URL + MIDNIGHT_INDEXER_URL");
        return;
    };

    let seed = WalletSeed::try_from_hex_str(DEV_WALLET_SEED).unwrap();
    let provider = MidnightProvider::new(&node_url, &indexer_url).expect("provider");
    let wallet = Wallet::sync(&provider, seed, Network::Undeployed)
        .await
        .expect("sync");
    let provider = provider.with_wallet(LocalWallet::new(wallet));

    // The wallet reads finalized state, so this deploy stays invisible to
    // any other wallet on this seed until it finalizes. The next test in the
    // suite is another process on the same seed, so the test waits for it.
    let (_, deployed) = counter::Contract::deploy(&provider)
        .with_initial_state(counter::LedgerInitialState::default())
        .with_zk_config(ZK_KEYS_DIR)
        .send()
        .await
        .expect("submit deploy")
        .wait_finalized()
        .await
        .expect("deploy finalized");
    let contract = deployed.into_contract().await.expect("deploy");

    // No wallet and no zk config: a build, a proof or a submit fails on either.
    let reader = MidnightProvider::new(&node_url, &indexer_url).expect("provider");
    let walletless = counter::Contract::at(&reader, contract.address()).build();

    let simulated = walletless
        .circuits()
        .increment()
        .simulate()
        .await
        .expect("a simulated call needs no wallet and no zk config");
    assert_eq!(simulated.value, 1, "increment returns 1");

    // A zero hash, or the hash of a block before the deploy, fails this read.
    let round_at_run = counter::Contract::at(&reader, contract.address())
        .at_block(NodeBlockHash::from(simulated.block_hash))
        .build()
        .ledger()
        .await
        .expect("the state at the block that the run read")
        .round()
        .expect("round");
    assert_eq!(
        simulated.ledger.round().expect("round"),
        round_at_run + 1,
        "the ledger must be the state after the call, not before it"
    );
}
