//! E2E tests against a running Midnight dev node (no indexer required).
//!
//! These tests require MIDNIGHT_NODE_URL to be set.
//! Run: MIDNIGHT_NODE_URL=ws://127.0.0.1:9944 cargo test --test node_e2e -- --show-output

use midnight_provider::{MidnightProvider, Provider, StateQuery};
use sp_storage::StorageKey;

fn node_only_provider() -> Option<MidnightProvider> {
    let node_url = std::env::var("MIDNIGHT_NODE_URL").ok()?;
    Some(MidnightProvider::new(&node_url, "http://127.0.0.1:1").expect("valid node URL"))
}

/// A provider wired to both the node and a real indexer, for the few tests
/// here that need ledger state rather than plain node RPC.
fn indexed_provider() -> Option<MidnightProvider> {
    let node_url = std::env::var("MIDNIGHT_NODE_URL").ok()?;
    let indexer_url = std::env::var("MIDNIGHT_INDEXER_URL").ok()?;
    Some(MidnightProvider::new(&node_url, &indexer_url).expect("valid URLs"))
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
async fn node_block_number() {
    let p = require_node!();
    let height = p.get_block_number().await.unwrap();
    eprintln!("block number: {height}");
}

#[tokio::test]
async fn node_chain_label() {
    let p = require_node!();
    // A human-readable chain-spec label, not the ledger network id. The
    // ledger network id needs the indexer and is covered by
    // `ledger_network_id_matches_the_network_we_sync_as`.
    let chain = p.system_chain().await.unwrap();
    assert!(!chain.is_empty());
    eprintln!("chain label: {chain}");
}

/// Every test and example in this repo syncs as [`Network::Undeployed`] and
/// derives bech32 addresses from that. Recipient parsing now rejects an address
/// whose HRP names a different network, so if the devnet's ledger ever reports
/// something else, that hardcoded choice is wrong and every transfer here would
/// start failing. Assert the two agree.
#[tokio::test]
async fn ledger_network_id_matches_the_network_we_sync_as() {
    let (Ok(_), Some(p)) = (std::env::var("MIDNIGHT_INDEXER_URL"), indexed_provider()) else {
        eprintln!("skipping: MIDNIGHT_NODE_URL or MIDNIGHT_INDEXER_URL not set");
        return;
    };
    let seed = midnight_provider::WalletSeed::try_from_hex_str(
        "0000000000000000000000000000000000000000000000000000000000000001",
    )
    .unwrap();
    let p = p
        .sync_wallet(seed, midnight_provider::Network::Undeployed)
        .await
        .expect("genesis wallet should sync against the devnet");

    let ledger_network_id = p.ledger_network_id().await.unwrap();
    let wallet_network = p.network().await.unwrap();
    eprintln!("ledger network id: {ledger_network_id}, wallet network: {wallet_network}");
    assert_eq!(
        wallet_network,
        midnight_provider::Network::from(ledger_network_id.as_str()),
        "wallet network and ledger network id disagree"
    );
}

#[tokio::test]
async fn node_health_connected() {
    let p = require_node!();
    let health = p.health().await.unwrap();
    assert!(health.node_connected);
    eprintln!("health: {health:?}");
}

// ---------------------------------------------------------------------------
// query_contract_state RPC endpoint availability
// ---------------------------------------------------------------------------

#[tokio::test]
async fn query_contract_state_nonexistent_contract() {
    let p = require_node!();
    let fake_address = "00".repeat(32);
    let result = p
        .query_contract_state(
            &fake_address,
            vec![StateQuery {
                path: vec![StorageKey(vec![0x40, 0x01])],
            }],
        )
        .await;

    match result {
        Err(e) => {
            let msg = e.to_string();
            eprintln!("expected error for nonexistent contract: {msg}");
            assert!(
                msg.contains("Unable") || msg.contains("error") || msg.contains("not found"),
                "unexpected error: {msg}"
            );
        }
        Ok(results) => {
            // Some implementations return results with per-query errors
            eprintln!("got results: {results:?}");
        }
    }
}

// ---------------------------------------------------------------------------
// Contract state query tests (require deployed contract)
//
// These are ignored by default — they need a running node with the test
// contract already deployed. Run manually:
//
//   MIDNIGHT_NODE_URL=ws://127.0.0.1:9944 \
//   MIDNIGHT_CONTRACT_ADDRESS=dd76bcd0...71577 \
//   cargo test --test node_e2e contract_deployed -- --ignored --show-output
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
