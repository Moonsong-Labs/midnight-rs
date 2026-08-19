//! The transaction hash the SDK reports is the one the indexer keys on.
//!
//! Substrate identifies the submitted extrinsic; the Midnight ledger
//! identifies the transaction inside it. The indexer stores only the second,
//! so a query by the first matches nothing. This submits one transaction and
//! checks both halves of that claim against a live indexer.
//!
//! Gated on a running devnet (`MIDNIGHT_NODE_URL`, `MIDNIGHT_INDEXER_URL`).

use std::time::Duration;

use midnight_provider::{
    MidnightProvider, NIGHT, Network, Seed, TransactionOffset, TxResultWait, WalletSeed,
};

const DEV_WALLET_SEED: &str = "0000000000000000000000000000000000000000000000000000000000000001";

#[tokio::test]
async fn the_indexer_keys_a_submitted_transaction_by_its_transaction_hash() {
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
    let transaction_hash_hex = pending.transaction_hash_hex();
    let (in_block, _pending) = pending.wait_finalized().await.expect("finalized");

    assert_eq!(
        in_block.transaction_hash, transaction_hash,
        "the hash must survive inclusion unchanged"
    );

    let outcome = provider
        .wait_transaction_result(
            transaction_hash,
            Duration::from_secs(60),
            Duration::from_secs(1),
        )
        .await
        .expect("polling the indexer must not error");
    let TxResultWait::Found(result) = outcome else {
        panic!("the indexer must surface a result for the hash the SDK reports");
    };
    eprintln!("indexed with status {:?}", result.status);

    let indexed = provider
        .get_transactions(TransactionOffset::hash(transaction_hash_hex.clone()))
        .await
        .expect("query by transaction hash");
    assert_eq!(
        indexed.first().map(|t| t.hash()),
        Some(transaction_hash_hex.as_str()),
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
