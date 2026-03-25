//! Integration tests against a running Midnight devnet indexer.
//! Skipped unless MIDNIGHT_INDEXER_URL is set.

use midnight_indexer_client::IndexerClient;

fn client() -> Option<IndexerClient> {
    std::env::var("MIDNIGHT_INDEXER_URL")
        .ok()
        .map(|url| IndexerClient::new(&url).expect("valid URL"))
}

macro_rules! require_indexer {
    () => {
        match client() {
            Some(c) => c,
            None => {
                eprintln!("skipping: MIDNIGHT_INDEXER_URL not set");
                return;
            }
        }
    };
}

#[tokio::test]
async fn health_check() {
    let client = require_indexer!();
    assert!(client.health_check().await);
}

#[tokio::test]
async fn get_latest_block() {
    let client = require_indexer!();
    let block = client.get_latest_block().await.unwrap();
    let block = block.expect("devnet should have blocks");
    assert!(block.height > 0);
    assert!(!block.hash.is_empty());
}

#[tokio::test]
async fn get_block_by_height() {
    let client = require_indexer!();
    let block = client.get_block_by_height(1).await.unwrap();
    let block = block.expect("block 1 should exist");
    assert_eq!(block.height, 1);
}

#[tokio::test]
async fn get_block_by_hash() {
    let client = require_indexer!();
    let latest = client.get_latest_block().await.unwrap().unwrap();
    let by_hash = client
        .get_block_by_hash(&latest.hash)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(by_hash.height, latest.height);
}

#[tokio::test]
async fn get_block_with_transactions() {
    let client = require_indexer!();
    let block = client
        .get_block_with_transactions(1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(block.height, 1);
    assert!(block.transactions.is_some());
}

#[tokio::test]
async fn get_nonexistent_block() {
    let client = require_indexer!();
    let block = client.get_block_by_height(999_999_999).await.unwrap();
    assert!(block.is_none());
}

#[tokio::test]
async fn get_transactions_by_nonexistent_hash() {
    let client = require_indexer!();
    let txs = client
        .get_transactions_by_hash(
            "0000000000000000000000000000000000000000000000000000000000000000",
        )
        .await
        .unwrap();
    assert!(txs.is_empty());
}
