//! `balance_transaction` on a **bare** fee-less contract call.
//!
//! The documented two-party flow is "party 1 builds fee-less, party 2 pays the
//! fees". When party 1 contributes only a contract call, with nothing merged
//! alongside it, the balanced transaction used to be rejected by the node with
//! `NotNormalized`: no Dust was drawn, yet an empty `DustActions` intent was
//! attached anyway, and the ledger treats that as non-canonical.
//!
//! `examples/combine-and-sponsor` never caught this because it merges the call
//! with a transfer before balancing.
//!
//! Gated on a running devnet (`MIDNIGHT_NODE_URL`, `MIDNIGHT_INDEXER_URL`).

mod counter {
    compact_bindgen::contract!("../../devnet/contracts/counter/compiled/contract-info.json");
}

use midnight_provider::{DustlessBuilder, MidnightProvider, Network, WalletSeed};

const ZK_KEYS_DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../devnet/contracts/counter/compiled"
);
const DEV_WALLET_SEED: &str = "0000000000000000000000000000000000000000000000000000000000000001";

#[tokio::test]
async fn balancing_a_bare_contract_call_is_accepted_on_chain() {
    let (Ok(node_url), Ok(indexer_url)) = (
        std::env::var("MIDNIGHT_NODE_URL"),
        std::env::var("MIDNIGHT_INDEXER_URL"),
    ) else {
        eprintln!("skipping: needs MIDNIGHT_NODE_URL + MIDNIGHT_INDEXER_URL");
        return;
    };

    let seed = WalletSeed::try_from_hex_str(DEV_WALLET_SEED).unwrap();
    let provider = MidnightProvider::new(&node_url, &indexer_url)
        .expect("provider")
        .sync_wallet(seed, Network::Undeployed)
        .await
        .expect("sync");

    let contract = counter::Contract::deploy(&provider)
        .with_initial_state(counter::LedgerInitialState::default())
        .with_zk_config(ZK_KEYS_DIR)
        .send()
        .await
        .expect("submit deploy")
        .into_contract()
        .await
        .expect("deploy");

    let round_before = contract
        .ledger()
        .await
        .expect("ledger")
        .round()
        .expect("round");

    // The bare case: one fee-less call, nothing merged into it.
    let dustless = contract
        .circuits()
        .increment()
        .without_dust()
        .await
        .expect("build fee-less call");

    let funded = provider
        .balance_transaction(dustless.as_bytes())
        .await
        .expect("balance the bare call");

    let pending = provider
        .submit(&funded)
        .await
        .expect("submit balanced call");
    let (in_block, _) = pending.wait_best().await.expect("the node must accept it");
    assert_eq!(
        in_block.verdict,
        midnight_provider::Verdict::Success,
        "the call's fallible phase must succeed"
    );

    // The call has to have actually run, not merely landed.
    let round_after = contract
        .ledger()
        .await
        .expect("ledger")
        .round()
        .expect("round");
    assert_eq!(
        round_after,
        round_before + 1,
        "the balanced call should have advanced the counter"
    );
}
