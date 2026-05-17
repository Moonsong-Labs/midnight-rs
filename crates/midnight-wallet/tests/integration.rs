//! Integration tests for midnight-wallet against a running devnet.
//!
//! These tests require a Midnight devnet node running at `ws://127.0.0.1:9944`.
//! Run with: `cargo test -p midnight-wallet --test integration -- --ignored`

use std::time::Duration;

use midnight_wallet::{Wallet, WalletBuilder, WalletState};

const DEV_SEED: &str = "0000000000000000000000000000000000000000000000000000000000000001";
const NODE_URL: &str = "ws://127.0.0.1:9944";

#[tokio::test]
#[ignore = "requires running devnet node"]
async fn sync_wallet_and_check_dust_balance() {
    let wallet = Wallet::from_seed_hex(DEV_SEED, "undeployed").unwrap();
    let state = WalletState::sync_from_node(NODE_URL, *wallet.seed())
        .await
        .expect("sync should succeed");

    let balance = state.balance();
    assert!(
        balance.dust.spendable_utxos > 0,
        "devnet faucet should have funded this wallet with DUST"
    );
    assert!(state.last_synced_height() > 0);
}

#[tokio::test]
#[ignore = "requires running devnet node"]
async fn resync_picks_up_new_blocks() {
    let wallet = Wallet::from_seed_hex(DEV_SEED, "undeployed").unwrap();
    let mut state = WalletState::sync_from_node(NODE_URL, *wallet.seed())
        .await
        .expect("initial sync should succeed");

    let initial_height = state.last_synced_height();

    // Wait for at least one new block (devnet produces ~2s blocks)
    tokio::time::sleep(Duration::from_secs(3)).await;

    let result = state.resync().await.expect("resync should succeed");
    assert!(
        result.height >= initial_height,
        "height should not decrease after resync"
    );
}

#[tokio::test]
#[ignore = "requires running devnet node"]
async fn live_wallet_background_sync() {
    let wallet = Wallet::from_seed_hex(DEV_SEED, "undeployed").unwrap();
    let live = WalletBuilder::new(wallet, NODE_URL)
        .sync_interval(Duration::from_secs(2))
        .build()
        .await
        .expect("build should succeed");

    let balance = live.balance().await;
    assert!(
        balance.dust.spendable_utxos > 0,
        "live wallet should show DUST balance after initial sync"
    );

    // Let background sync tick at least once
    tokio::time::sleep(Duration::from_secs(3)).await;

    live.shutdown().await;
}

#[tokio::test]
#[ignore = "requires running devnet node"]
async fn shielded_balance_accessible() {
    let wallet = Wallet::from_seed_hex(DEV_SEED, "undeployed").unwrap();
    let state = WalletState::sync_from_node(NODE_URL, *wallet.seed())
        .await
        .expect("sync should succeed");

    let balance = state.balance();
    // Shielded balance may be 0 on a fresh devnet, but the query should not panic
    let _ = balance.shielded.total_count;
    let _ = balance.shielded.coins;
}

#[tokio::test]
#[ignore = "requires running devnet node"]
async fn deploy_funded_with_state_skips_full_resync() {
    use midnight_wallet::WalletState;
    use std::time::Instant;

    let wallet = Wallet::from_seed_hex(DEV_SEED, "undeployed").unwrap();

    // First: full sync (baseline timing)
    let start = Instant::now();
    let state = WalletState::sync_from_node(NODE_URL, *wallet.seed())
        .await
        .expect("sync");
    let full_sync_time = start.elapsed();

    // Second: use cached state (should be near-instant)
    let start = Instant::now();
    let context = midnight_contract::sync_or_fetch_context(
        Some(&state),
        NODE_URL,
        *wallet.seed(),
    )
    .await
    .expect("sync_or_fetch_context with cached state");
    let cached_time = start.elapsed();

    assert!(
        cached_time < full_sync_time / 2,
        "cached path ({cached_time:?}) should be much faster than full sync ({full_sync_time:?})"
    );
    drop(context);
}
