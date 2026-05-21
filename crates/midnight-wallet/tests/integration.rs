//! Integration tests for midnight-wallet against a running devnet.
//!
//! These tests require MIDNIGHT_NODE_URL and MIDNIGHT_INDEXER_URL to be set.
//! The CI runs a devnet (node + indexer) via docker compose.
//!
//! Run locally:
//!   MIDNIGHT_NODE_URL=ws://127.0.0.1:9944 MIDNIGHT_INDEXER_URL=http://127.0.0.1:8088 \
//!     cargo test -p midnight-wallet --test integration -- --show-output

use midnight_node_ledger_helpers::WalletSeed;
use midnight_wallet::Wallet;

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

    let wallet = Wallet::sync(&node, &indexer, dev_seed(), "undeployed", None)
        .await
        .expect("indexer sync should succeed");

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

    let wallet = Wallet::sync(&node, &indexer, seed, "undeployed", None)
        .await
        .expect("indexer sync should succeed");

    let provider = midnight_provider::MidnightProvider::new(&node, &indexer)
        .expect("provider construction")
        .with_wallet(wallet);

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

    let wallet = Wallet::sync(&node, &indexer, seed, "undeployed", None)
        .await
        .expect("indexer sync should succeed");

    let provider = midnight_provider::MidnightProvider::new(&node, &indexer)
        .expect("provider construction")
        .with_wallet(wallet);

    let balance = provider.balance().await.expect("wallet attached");
    eprintln!(
        "pre-transfer balance: dust={}, shielded={}",
        balance.dust.spendable_utxos, balance.shielded.total_count,
    );

    let context = provider
        .build_context()
        .await
        .expect("build_context should succeed");

    let proof_provider: std::sync::Arc<
        dyn midnight_node_ledger_helpers::ProofProvider<midnight_node_ledger_helpers::DefaultDB>,
    > = std::sync::Arc::new(midnight_node_ledger_helpers::LocalProofServer::new());

    let wallet_arc = provider.wallet().expect("wallet attached");
    let result = {
        let w = wallet_arc.read().await;
        let transfer = midnight_wallet::TransferBuilder::new(&w, context.clone(), proof_provider);
        transfer
            .shielded(
                midnight_node_ledger_helpers::ShieldedTokenType(
                    midnight_node_ledger_helpers::HashOutput([0u8; 32]),
                ),
                1,
                seed,
            )
            .await
    };

    match &result {
        Ok(tx_result) => eprintln!(
            "transfer built successfully, tx_bytes={}",
            tx_result.tx_bytes.len()
        ),
        Err(e) => eprintln!("transfer failed: {e}"),
    }

    let tx_result = result.expect("shielded transfer should build successfully");

    // Submit and finalize so subsequent tests don't try to double-spend the
    // same dust UTXOs.
    let pending = midnight_contract::call::submit(&node, &tx_result.tx_bytes)
        .await
        .expect("transaction submission should succeed");
    eprintln!("transaction submitted: {}", pending.extrinsic_hash_hex());
    let (_best, pending) = pending.wait_best().await.expect("wait_best");
    let (_finalized, _) = pending.wait_finalized().await.expect("wait_finalized");
    eprintln!("transaction finalized");
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
