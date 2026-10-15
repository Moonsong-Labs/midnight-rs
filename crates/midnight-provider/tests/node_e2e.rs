//! E2E tests against a running Midnight dev node (no indexer required).
//!
//! These tests require MIDNIGHT_NODE_URL to be set.
//! Run: MIDNIGHT_NODE_URL=ws://127.0.0.1:9944 cargo test --test node_e2e -- --show-output

#[expect(dead_code, reason = "this test needs only the skip")]
mod common;

use midnight_provider::{MidnightProvider, Network, Provider, StateQuery};

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

// ---------------------------------------------------------------------------
// Contract state query tests (require deployed contract)
//
// These skip unless MIDNIGHT_CONTRACT_ADDRESS is set. They need a running node
// that serves `midnight_queryContractState`, with the test contract already
// deployed. Run manually:
//
//   MIDNIGHT_NODE_URL=ws://127.0.0.1:9944 \
//   MIDNIGHT_CONTRACT_ADDRESS=dd76bcd0...71577 \
//   cargo test --test node_e2e contract_deployed -- --show-output
// ---------------------------------------------------------------------------

mod contract_deployed {
    use super::*;
    use compact_bindgen::{cell_as, lazy};

    fn contract_address() -> Option<String> {
        let Ok(address) = std::env::var("MIDNIGHT_CONTRACT_ADDRESS") else {
            eprintln!("skipping: MIDNIGHT_CONTRACT_ADDRESS not set");
            return None;
        };
        Some(address)
    }

    #[tokio::test]
    async fn query_counter_field() {
        let Some(p) = node_only_provider() else {
            return;
        };
        let Some(address) = contract_address() else {
            return;
        };

        // State: Array(1) [ Array(3) [ MerkleTree(10), Cell(counter), Map{...} ] ]
        // Path [0][1] → Cell(u64)
        let results = p
            .query_contract_state(
                &address,
                vec![StateQuery {
                    path: vec![lazy::index_to_query_key(0), lazy::index_to_query_key(1)],
                }],
            )
            .await
            .expect("query_contract_state failed");

        assert_eq!(results.len(), 1);
        assert!(
            results[0].error.is_none(),
            "query error: {:?}",
            results[0].error
        );

        let counter: u64 =
            cell_as(&lazy::decode_state_value(&results[0]).expect("decode the state value"))
                .expect("u64 Cell");

        eprintln!("counter value: {counter}");
    }

    #[tokio::test]
    async fn query_batch_with_error() {
        let Some(p) = node_only_provider() else {
            return;
        };
        let Some(address) = contract_address() else {
            return;
        };

        let results = p
            .query_contract_state(
                &address,
                vec![
                    StateQuery {
                        path: vec![lazy::index_to_query_key(0), lazy::index_to_query_key(1)],
                    },
                    StateQuery {
                        path: vec![lazy::index_to_query_key(0), lazy::index_to_query_key(99)],
                    },
                ],
            )
            .await
            .expect("batch query failed");

        assert_eq!(results.len(), 2);

        // First query: valid field
        assert!(results[0].value.is_some());
        assert!(results[0].error.is_none());

        // Second query: out of bounds
        assert!(results[1].value.is_none());
        assert!(
            results[1].error.as_ref().unwrap().contains("out of bounds"),
            "expected out-of-bounds error, got: {:?}",
            results[1].error
        );

        eprintln!("batch query: field ok, out-of-bounds error ok");
    }
}
