//! A contract call runs its circuit at the time of the node's best block, or
//! of the block of its `at_block` pin, and sees the contract's balance.
//!
//! The local run and the partition of the call's transcripts both read the
//! block time and the balance. The chain checks each read again when it
//! applies the call. So the chain accepts a call that reads the clock or the
//! balance only when the SDK uses the chain's values at each step.
//!
//! Gated on a running devnet (`MIDNIGHT_NODE_URL`, `MIDNIGHT_INDEXER_URL`).
//! Under `make test-e2e`, which sets `MIDNIGHT_E2E`, a missing URL panics.

mod common;

mod call_context {
    compact_bindgen::contract!(
        "../../devnet/contracts/call-context/compiled/compiler/analyzed-ir.sexp"
    );
}

use compact_bindgen::Bytes;
use midnight_contract::DustlessBuilder;
use midnight_provider::MidnightProvider;

const DOMAIN_SEP: [u8; 32] = [0x5c; 32];
const MINTED: u64 = 1_000;

#[tokio::test]
async fn a_call_runs_at_the_best_block_time_and_sees_the_contract_balance() {
    let Some((node_url, indexer_url)) = common::devnet_urls() else {
        return;
    };
    assert!(
        std::path::Path::new(call_context::ZK_CONFIG_DIR)
            .join("keys")
            .is_dir(),
        "{}/keys is missing; run `make compile-contracts CONTRACTS=call-context`",
        call_context::ZK_CONFIG_DIR
    );

    let provider = MidnightProvider::new(&node_url, &indexer_url);
    let wallet = common::synced_wallet(&provider, common::DEV_WALLET_SEED).await;
    let provider = provider.with_wallet(wallet);

    let contract = call_context::Contract::deploy(&provider)
        .with_initial_state(call_context::LedgerInitialState::default())
        .with_zk_config(call_context::ZK_CONFIG_DIR)
        .await
        .expect("deploy");

    // Read the best block's time by its height, apart from the read that the
    // call path makes, so a call path on the wrong block disagrees with it.
    let best = provider.get_block_number().await.expect("best height");
    let hash = *provider
        .get_block_hashes_by_height(best)
        .await
        .expect("hashes at the best height")
        .first()
        .expect("the best height has a block");
    let now = provider
        .get_block_timestamp(hash)
        .await
        .expect("best block time")
        .as_secs();

    // An older block, such as the finalized head, makes `after` fail locally
    // with "too early". A time in milliseconds makes `before` fail with "too
    // late".
    contract
        .circuits()
        .unlock(now - 1, now + 3600)
        .await
        .expect("an unlock between the best block's time and an hour later must apply");
    let unlocked = contract
        .ledger()
        .await
        .expect("ledger")
        .unlocked()
        .expect("unlocked");
    assert_eq!(unlocked, 1, "the unlock must have run on chain");

    // The unlock above landed after `hash`, so the best block's time is past
    // `now + 1`. Only a call that runs at the time of its pin passes `before`.
    call_context::Contract::at(&provider, contract.address())
        .with_zk_config(call_context::ZK_CONFIG_DIR)
        .at_block(hash)
        .build()
        .circuits()
        .unlock(now - 1, now + 1)
        .without_dust()
        .await
        .expect("a call pinned at the block of `now` must run at that block's time");

    let color = contract
        .circuits()
        .mint(Bytes(DOMAIN_SEP), MINTED)
        .await
        .expect("the contract must be able to mint to itself")
        .value;
    contract
        .circuits()
        .holds(color, u128::from(MINTED) - 1)
        .await
        .expect("the contract must see the balance that its mint gave it");
}
