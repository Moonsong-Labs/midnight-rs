//! Integration tests against a running Midnight devnet indexer.
//! Skipped unless MIDNIGHT_INDEXER_URL is set.
//! Under `make test-e2e`, which sets `MIDNIGHT_E2E`, a missing URL panics.

use midnight_indexer_client::{BlockOffset, IndexerClient};

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
                if std::env::var_os("MIDNIGHT_E2E").is_some() {
                    panic!("MIDNIGHT_INDEXER_URL is missing while MIDNIGHT_E2E is set");
                }
                eprintln!("skipping: MIDNIGHT_INDEXER_URL not set");
                return;
            }
        }
    };
}

#[tokio::test]
async fn get_block_by_hash() {
    let client = require_indexer!();
    let latest = client.get_block(None).await.unwrap().unwrap();
    assert!(latest.height > 0, "the latest block must be past genesis");
    let by_hash = client
        .get_block(Some(BlockOffset::hash(&latest.hash)))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(by_hash.height, latest.height);
}

#[tokio::test]
async fn get_nonexistent_block() {
    let client = require_indexer!();
    let block = client
        .get_block(Some(BlockOffset::height(999_999_999)))
        .await
        .unwrap();
    assert!(block.is_none());
}
