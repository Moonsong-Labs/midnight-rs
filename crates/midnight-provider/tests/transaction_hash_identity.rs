//! The transaction hash the SDK reports is the ledger's own identity for the
//! transaction.
//!
//! Substrate identifies the submitted extrinsic; the Midnight ledger
//! identifies the transaction inside it. The SDK computes the second one
//! locally, by hashing the bytes it submits, and every handle carries it. This
//! submits one transaction and checks that value against a live chain: the
//! `Midnight::TxApplied` event of its inclusion block names exactly it.
//!
//! Gated on a running devnet (`MIDNIGHT_NODE_URL`, `MIDNIGHT_INDEXER_URL`).

#[expect(dead_code, reason = "this test needs no synced_provider")]
mod common;

use midnight_provider::{MidnightProvider, NIGHT, NodeBlockHash};

#[tokio::test]
async fn the_hash_the_sdk_computes_is_the_one_the_chain_uses() {
    let Some((node_url, indexer_url)) = common::devnet_urls() else {
        return;
    };

    let provider = MidnightProvider::new(&node_url, &indexer_url);
    let wallet = common::synced_wallet(&provider, common::DEV_WALLET_SEED).await;
    let recipient = wallet.unshielded_address();
    let provider = provider.with_wallet(wallet);

    // A self-transfer is the cheapest transaction that reaches a block.
    let pending = provider
        .transfer_unshielded(NIGHT, 1, &recipient)
        .await
        .expect("submit self-transfer");
    let transaction_hash = pending.transaction_hash();
    let (in_block, _pending) = pending
        .wait_finalized()
        .await
        .expect("finalized and applied");

    assert_eq!(
        in_block.transaction_hash, transaction_hash,
        "the hash must survive inclusion unchanged"
    );

    // The chain names the same value in the event it emits for the
    // transaction, which is what an explorer or an indexer keys on.
    let events = subxt::OnlineClient::<subxt::SubstrateConfig>::from_url(&node_url)
        .await
        .expect("connect to the node")
        .at_block(NodeBlockHash::from(in_block.block_hash))
        .await
        .expect("the inclusion block")
        .events()
        .fetch()
        .await
        .expect("the events of the inclusion block");
    let applied: Vec<[u8; 32]> = events
        .iter()
        .map(|event| event.expect("decode an event"))
        .filter(|event| event.pallet_name() == "Midnight" && event.event_name() == "TxApplied")
        .map(|event| {
            <[u8; 32]>::try_from(event.field_bytes()).expect("TxApplied holds one 32-byte hash")
        })
        .collect();
    assert!(
        applied.contains(transaction_hash.as_bytes()),
        "the chain's TxApplied must name the hash the SDK computed, got {applied:x?}"
    );
}
