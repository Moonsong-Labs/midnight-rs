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

    let wallet = provider
        .wallet()
        .await
        .expect("wallet attached after sync_wallet");
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

    let balance = provider
        .balance()
        .await
        .expect("wallet attached after sync_wallet");
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

/// Exercises the shielded transfer build path with a non-zero shielded token
/// id. The existing `build_shielded_transfer` uses the all-zero token id
/// `[0; 32]`, which is just the conventional default the dev preset mints; a
/// future change that quietly short-circuits coin selection for that default
/// would still pass that test. This test picks a different shielded token at
/// runtime (the dev preset mints a few) and asserts the build path handles
/// it identically. Skips if only the zero-id token is held.
///
/// (NIGHT is the chain's native *unshielded* token and lives in
/// `WalletBalance::unshielded`; there is no shielded NIGHT, so the property
/// here is purely about token-id genericity in the shielded path.)
///
/// We deliberately stop at build (no submit) for two reasons: (a) the
/// pre-allocated non-default dev tokens have chain-side transfer restrictions
/// (custom error 171 observed) so the chain will reject the tx after
/// inclusion, and (b) submitting would pollute the mempool with dust spends
/// that conflict with `build_shielded_transfer` running in parallel. Build
/// success — proof generation, offer construction, and tagged serialization
/// — is enough to pin the property.
#[tokio::test]
async fn build_shielded_transfer_arbitrary_token_id() {
    let (node, indexer) = require_devnet!();
    let seed = dev_seed();

    let provider = MidnightProvider::new(&node, &indexer)
        .expect("provider construction")
        .sync_wallet(seed.clone(), "undeployed", None)
        .await
        .expect("indexer sync should succeed");

    let zero_token_id_hex = "0".repeat(64);
    let balance = provider
        .balance()
        .await
        .expect("wallet attached after sync_wallet");
    let Some(coin) = balance
        .shielded
        .coins
        .iter()
        .find(|c| c.token_type != zero_token_id_hex)
        .cloned()
    else {
        eprintln!("skipping: dev wallet has no shielded coins with a non-zero token id");
        return;
    };
    eprintln!(
        "shielded coin with non-zero token id: token={} value={}",
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
        .expect("shielded transfer of arbitrary token id should build (proofs + serialize)");
    eprintln!(
        "shielded transfer built, tx_bytes={}",
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
