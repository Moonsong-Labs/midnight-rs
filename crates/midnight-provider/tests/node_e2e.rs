//! E2E tests against a running Midnight dev node (no indexer required).
//!
//! These tests require MIDNIGHT_NODE_URL to be set.
//! Run: MIDNIGHT_NODE_URL=ws://127.0.0.1:9944 cargo test --test node_e2e -- --show-output

#[expect(dead_code, reason = "this test needs only the skip")]
mod common;

use midnight_provider::{MidnightProvider, Network};

fn node_only_provider() -> Option<MidnightProvider> {
    let Ok(node_url) = std::env::var("MIDNIGHT_NODE_URL") else {
        common::skip("MIDNIGHT_NODE_URL");
        return None;
    };
    Some(MidnightProvider::new(&node_url, "http://127.0.0.1:1"))
}

// ---------------------------------------------------------------------------
// Node connectivity
// ---------------------------------------------------------------------------

#[tokio::test]
async fn node_chain_label() {
    let Some(p) = node_only_provider() else {
        return;
    };
    let chain = p.system_chain().await.unwrap();
    assert!(!chain.is_empty());
    eprintln!("chain label: {chain}");
}

/// The provider has no wallet, so only the node's runtime can answer.
#[tokio::test]
async fn the_node_runs_the_network_the_tests_sync_as() {
    let Some(p) = node_only_provider() else {
        return;
    };
    let network = p.ledger_network_id().await.unwrap();
    assert_eq!(network, Some(Network::Undeployed));
}
