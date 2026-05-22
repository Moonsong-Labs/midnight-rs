//! Integration tests for midnight-wallet against a running devnet.
//!
//! These tests require MIDNIGHT_NODE_URL and MIDNIGHT_INDEXER_URL to be set.
//! The CI runs a devnet (node + indexer) via docker compose.
//!
//! Run locally:
//!   MIDNIGHT_NODE_URL=ws://127.0.0.1:9944 MIDNIGHT_INDEXER_URL=http://127.0.0.1:8088 \
//!     cargo test -p midnight-wallet --test integration -- --show-output

use midnight_provider::MidnightProvider;
use midnight_wallet::WalletSeed;

const DEV_SEED: &str = "0000000000000000000000000000000000000000000000000000000000000001";

fn dev_seed() -> WalletSeed {
    WalletSeed::try_from_hex_str(DEV_SEED).unwrap()
}

fn node_url() -> Option<String> {
    std::env::var("MIDNIGHT_NODE_URL").ok()
}

fn indexer_url() -> Option<String> {
    std::env::var("MIDNIGHT_INDEXER_URL").ok()
}

macro_rules! require_devnet {
    () => {{
        let node = match node_url() {
            Some(u) => u,
            None => {
                if std::env::var("MIDNIGHT_E2E").is_ok() {
                    panic!("MIDNIGHT_NODE_URL must be set in CI");
                }
                eprintln!("skipping: MIDNIGHT_NODE_URL not set");
                return;
            }
        };
        let indexer = match indexer_url() {
            Some(u) => u,
            None => {
                if std::env::var("MIDNIGHT_E2E").is_ok() {
                    panic!("MIDNIGHT_INDEXER_URL must be set in CI");
                }
                eprintln!("skipping: MIDNIGHT_INDEXER_URL not set");
                return;
            }
        };
        (node, indexer)
    }};
}

// ---------------------------------------------------------------------------
// Indexer-based sync (zswap + dust + unshielded events)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sync_replays_events() {
    let (node, indexer) = require_devnet!();

    let provider = MidnightProvider::new(&node, &indexer)
        .expect("provider construction")
        .sync_wallet(dev_seed(), "undeployed", None)
        .await
        .expect("indexer sync should succeed");

    let wallet = provider.wallet_read().await.expect("wallet attached");
    eprintln!(
        "synced: height={}, utxos={}, zswap_event_id={}, dust_event_id={}",
        wallet.last_block_height(),
        wallet.unshielded_utxos().len(),
        wallet.zswap_event_id(),
        wallet.dust_event_id(),
    );

    assert!(
        wallet.last_tx_id().is_some(),
        "expected last_tx_id to be set after sync"
    );
    assert!(
        wallet.zswap_event_id() > 0,
        "expected zswap events to have been replayed"
    );
    assert!(
        wallet.dust_event_id() > 0,
        "expected dust events to have been replayed"
    );
}

// ---------------------------------------------------------------------------
// Build context from indexed state via the provider
// ---------------------------------------------------------------------------

#[tokio::test]
async fn provider_build_context_succeeds() {
    let (node, indexer) = require_devnet!();
    let seed = dev_seed();

    let provider = MidnightProvider::new(&node, &indexer)
        .expect("provider construction")
        .sync_wallet(seed.clone(), "undeployed", None)
        .await
        .expect("indexer sync should succeed");

    let context = provider
        .build_context()
        .await
        .expect("build_context should succeed");

    let wallets = context.wallets.lock().unwrap();
    assert!(
        wallets.contains_key(&seed),
        "context should contain our wallet"
    );
}

// ---------------------------------------------------------------------------
// Transfer transaction building via the provider's wallet
// ---------------------------------------------------------------------------

#[tokio::test]
async fn build_shielded_transfer() {
    let (node, indexer) = require_devnet!();
    let seed = dev_seed();

    let provider = MidnightProvider::new(&node, &indexer)
        .expect("provider construction")
        .sync_wallet(seed.clone(), "undeployed", None)
        .await
        .expect("indexer sync should succeed");

    let balance = provider.balance().await.expect("wallet attached");
    eprintln!(
        "pre-transfer balance: dust={}, shielded={}",
        balance.dust.spendable_utxos, balance.shielded.total_count,
    );

    let recipient = midnight_wallet::address::derive_shielded(&seed, "undeployed");
    let tx_result = provider
        .transfer_shielded(
            midnight_helpers::ShieldedTokenType(midnight_helpers::HashOutput([0u8; 32])),
            1,
            &recipient,
        )
        .await
        .expect("shielded transfer should build successfully");

    eprintln!(
        "transfer built successfully, tx_bytes={}",
        tx_result.tx_bytes.len()
    );

    // Submit and finalize so subsequent tests don't try to double-spend the
    // same dust UTXOs.
    let pending = provider
        .submit(&tx_result.tx_bytes)
        .await
        .expect("transaction submission should succeed");
    eprintln!("transaction submitted: {}", pending.extrinsic_hash_hex());
    let (_best, pending) = pending.wait_best().await.expect("wait_best");
    let (_finalized, _) = pending.wait_finalized().await.expect("wait_finalized");
    eprintln!("transaction finalized");
}

/// Exercises the shielded transfer build path with a non-NIGHT token type.
/// The dev preset of the midnight-node image pre-funds the dev seed with
/// several shielded token types; we discover one at runtime rather than
/// hardcoding a token id, and skip if the wallet only holds NIGHT.
///
/// We deliberately stop at build (no submit) for two reasons: (a) the
/// pre-allocated non-NIGHT dev tokens have chain-side transfer restrictions
/// (custom error 171 observed) so the chain will reject the tx after
/// inclusion, and (b) submitting would pollute the mempool with dust spends
/// that conflict with the NIGHT-side `build_shielded_transfer` test running
/// in parallel. Build success is the property we pin — proof generation,
/// serialization, and offer construction all run during build, so success
/// proves the wallet path handles arbitrary `ShieldedTokenType` and isn't
/// quietly special-casing the zero (NIGHT) token id.
#[tokio::test]
async fn build_shielded_transfer_non_night_token() {
    let (node, indexer) = require_devnet!();
    let seed = dev_seed();

    let provider = MidnightProvider::new(&node, &indexer)
        .expect("provider construction")
        .sync_wallet(seed.clone(), "undeployed", None)
        .await
        .expect("indexer sync should succeed");

    let night_hex = "0".repeat(64);
    let balance = provider.balance().await.expect("wallet attached");
    let Some(coin) = balance
        .shielded
        .coins
        .iter()
        .find(|c| c.token_type != night_hex)
        .cloned()
    else {
        eprintln!("skipping: dev wallet has no non-NIGHT shielded coins");
        return;
    };
    eprintln!(
        "non-NIGHT shielded coin: token={} value={}",
        coin.token_type, coin.value
    );

    let token_bytes: [u8; 32] = hex::decode(&coin.token_type)
        .expect("token hex decodes")
        .try_into()
        .expect("token bytes are 32 long");
    let token_type = midnight_helpers::ShieldedTokenType(midnight_helpers::HashOutput(token_bytes));

    let recipient = midnight_wallet::address::derive_shielded(&seed, "undeployed");
    let tx_result = provider
        .transfer_shielded(token_type, 1, &recipient)
        .await
        .expect("non-NIGHT shielded transfer should build (proofs + serialize)");
    eprintln!(
        "non-NIGHT transfer built, tx_bytes={}",
        tx_result.tx_bytes.len()
    );
    assert!(
        tx_result.tx_bytes.len() > 1000,
        "tx bytes too small to be a real proven shielded transfer ({})",
        tx_result.tx_bytes.len()
    );
}

// ---------------------------------------------------------------------------
// Subscription client connectivity
// ---------------------------------------------------------------------------

#[tokio::test]
async fn subscription_client_connects() {
    let (_node, indexer) = require_devnet!();

    let sub_client = midnight_indexer_client::SubscriptionClient::new(&indexer);

    let variables = serde_json::json!({ "offset": { "height": 0 } });
    let mut subscription = sub_client
        .subscribe::<serde_json::Value>(
            midnight_indexer_client::subscription::queries::BLOCKS_SUBSCRIPTION,
            variables,
        )
        .await
        .expect("blocks subscription should connect");

    let event = tokio::time::timeout(std::time::Duration::from_secs(10), subscription.next())
        .await
        .expect("should receive block within 10s");

    assert!(
        event.is_some(),
        "subscription should yield at least one event"
    );
    let event = event.unwrap().expect("event should be Ok");
    eprintln!("received block event: {event}");
}
