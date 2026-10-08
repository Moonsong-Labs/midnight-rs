//! E2E tests against a running Midnight dev node (no indexer required).
//!
//! These tests require MIDNIGHT_NODE_URL to be set.
//! Run: MIDNIGHT_NODE_URL=ws://127.0.0.1:9944 cargo test --test node_e2e -- --show-output

use midnight_provider::{MidnightProvider, Network, Provider, StateQuery};
use sp_storage::StorageKey;

fn node_only_provider() -> Option<MidnightProvider> {
    let node_url = std::env::var("MIDNIGHT_NODE_URL").ok()?;
    Some(MidnightProvider::new(&node_url, "http://127.0.0.1:1").expect("valid node URL"))
}

macro_rules! require_node {
    () => {
        match node_only_provider() {
            Some(p) => p,
            None => {
                eprintln!("skipping: MIDNIGHT_NODE_URL not set");
                return;
            }
        }
    };
}

// ---------------------------------------------------------------------------
// Node connectivity
// ---------------------------------------------------------------------------

#[tokio::test]
async fn node_chain_label() {
    let p = require_node!();
    let chain = p.system_chain().await.unwrap();
    assert!(!chain.is_empty());
    eprintln!("chain label: {chain}");
}

/// The provider has no wallet, so only the node's runtime can answer.
#[tokio::test]
async fn the_node_runs_the_network_the_tests_sync_as() {
    let p = require_node!();
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
    use compact_bindgen::{InMemoryDB, StateValue, cell_value, hex, lazy, tagged_deserialize};

    fn contract_address() -> Option<String> {
        std::env::var("MIDNIGHT_CONTRACT_ADDRESS").ok()
    }

    fn field_key(index: usize) -> StorageKey {
        StorageKey(hex::decode(lazy::index_to_query_key(index)).unwrap())
    }

    #[tokio::test]
    async fn query_counter_field() {
        let p = require_node!();
        let address = match contract_address() {
            Some(a) => a,
            None => {
                eprintln!("skipping: MIDNIGHT_CONTRACT_ADDRESS not set");
                return;
            }
        };

        // State: Array(1) [ Array(3) [ MerkleTree(10), Cell(counter), Map{...} ] ]
        // Path [0][1] → Cell(u64)
        let results = p
            .query_contract_state(
                &address,
                vec![StateQuery {
                    path: vec![field_key(0), field_key(1)],
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

        let hex_value = results[0].value.as_ref().expect("expected a value");
        let bytes = hex::decode(hex_value).expect("valid hex");
        let sv: StateValue<InMemoryDB> =
            tagged_deserialize(&mut &bytes[..]).expect("deserialize StateValue");
        let av = cell_value(&sv).expect("expected Cell");
        let counter = u64::try_from(&*av.value).expect("u64 from AlignedValue");

        eprintln!("counter value: {counter}");
    }

    #[tokio::test]
    async fn query_batch_with_error() {
        let p = require_node!();
        let address = match contract_address() {
            Some(a) => a,
            None => {
                eprintln!("skipping: MIDNIGHT_CONTRACT_ADDRESS not set");
                return;
            }
        };

        let results = p
            .query_contract_state(
                &address,
                vec![
                    StateQuery {
                        path: vec![field_key(0), field_key(1)],
                    },
                    StateQuery {
                        path: vec![field_key(0), field_key(99)],
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
