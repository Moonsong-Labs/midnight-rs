//! The transaction hash the SDK reports is the ledger's own identity for the
//! transaction.
//!
//! Substrate identifies the submitted extrinsic; the Midnight ledger
//! identifies the transaction inside it. The SDK computes the second one
//! locally, by hashing the bytes it submits, and every handle carries it. This
//! submits one transaction and checks that value against a live chain: the
//! indexer stores exactly it, and stores no extrinsic hash at all.
//!
//! Gated on a running devnet (`MIDNIGHT_NODE_URL`, `MIDNIGHT_INDEXER_URL`).

use std::time::Duration;

use midnight_provider::{
    MidnightProvider, NIGHT, Network, Seed, TransactionOffset, Verdict, WalletSeed,
};

const DEV_WALLET_SEED: &str = "0000000000000000000000000000000000000000000000000000000000000001";

#[tokio::test]
async fn the_hash_the_sdk_computes_is_the_one_the_chain_uses() {
    let (Ok(node_url), Ok(indexer_url)) = (
        std::env::var("MIDNIGHT_NODE_URL"),
        std::env::var("MIDNIGHT_INDEXER_URL"),
    ) else {
        eprintln!("skipping: needs MIDNIGHT_NODE_URL + MIDNIGHT_INDEXER_URL");
        return;
    };

    let seed = Seed::from_hex(DEV_WALLET_SEED).expect("dev seed");
    let network = Network::Undeployed;
    let recipient = seed.unshielded_address(&network);
    let provider = MidnightProvider::new(&node_url, &indexer_url)
        .expect("provider")
        .sync_wallet(WalletSeed::from(seed), network)
        .await
        .expect("sync");

    // A self-transfer is the cheapest transaction that reaches a block.
    let pending = provider
        .transfer_unshielded(NIGHT, 1, &recipient)
        .await
        .expect("submit self-transfer");
    let transaction_hash = pending.transaction_hash();
    let (in_block, _pending) = pending.wait_finalized().await.expect("finalized");

    assert_eq!(
        in_block.transaction_hash, transaction_hash,
        "the hash must survive inclusion unchanged"
    );
    // The node's own events carry the verdict, so nothing here needs the
    // indexer to learn what the transaction did.
    assert_eq!(in_block.verdict, Verdict::Success);

    // The indexer stores the same value, which is what makes a query keyed by
    // transaction hash resolve.
    let mut indexed = Vec::new();
    for _ in 0..60 {
        indexed = provider
            .get_transactions(TransactionOffset::hash(transaction_hash.to_string()))
            .await
            .expect("query by transaction hash");
        if !indexed.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    assert_eq!(
        indexed.first().map(|t| t.hash()),
        Some(transaction_hash.to_string().as_str()),
        "the indexer's own hash must equal the one the SDK computed"
    );

    let by_extrinsic = provider
        .get_transactions(TransactionOffset::hash(hex::encode(
            in_block.extrinsic_hash,
        )))
        .await
        .expect("query by extrinsic hash");
    assert!(
        by_extrinsic.is_empty(),
        "the indexer holds no extrinsic hash, so this offset must match nothing"
    );
}
