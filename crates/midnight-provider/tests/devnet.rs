//! Integration tests against a running Midnight devnet.
//! Skipped unless MIDNIGHT_INDEXER_URL and MIDNIGHT_NODE_URL are set.

use midnight_provider::{MidnightProvider, Provider, StateQuery};

fn provider() -> Option<MidnightProvider> {
    let indexer_url = std::env::var("MIDNIGHT_INDEXER_URL").ok()?;
    let node_url = std::env::var("MIDNIGHT_NODE_URL").ok()?;
    Some(MidnightProvider::new(&node_url, &indexer_url).expect("valid URLs"))
}

fn contract_address() -> Option<String> {
    if let Ok(addr) = std::env::var("MIDNIGHT_CONTRACT_ADDRESS") {
        return Some(addr);
    }
    if let Ok(path) = std::env::var("MIDNIGHT_CONTRACT_ADDRESS_FILE") {
        if let Ok(contents) = std::fs::read_to_string(&path) {
            let addr = contents.trim().to_string();
            if !addr.is_empty() {
                return Some(addr);
            }
        }
    }
    None
}

macro_rules! require_provider {
    () => {
        match provider() {
            Some(p) => p,
            None => {
                eprintln!("skipping: MIDNIGHT_INDEXER_URL or MIDNIGHT_NODE_URL not set");
                return;
            }
        }
    };
}

macro_rules! require_contract {
    () => {{
        let p = require_provider!();
        match contract_address() {
            Some(addr) => (p, addr),
            None => {
                eprintln!("skipping: MIDNIGHT_CONTRACT_ADDRESS not set");
                return;
            }
        }
    }};
}

#[tokio::test]
async fn health_check() {
    let p = require_provider!();
    let health = p.health().await.unwrap();
    assert!(health.node_connected);
    assert!(health.indexer_connected);
    assert!(health.block_height.unwrap() > 0);
    eprintln!("health: {health:?}");
}

#[tokio::test]
async fn get_block_number() {
    let p = require_provider!();
    let height = p.get_block_number().await.unwrap();
    assert!(height > 0);
}

#[tokio::test]
async fn get_block() {
    let p = require_provider!();
    let block = p.get_block().await.unwrap().unwrap();
    assert!(block.height > 0);
}

#[tokio::test]
async fn get_contract_state() {
    let (p, addr) = require_contract!();
    let hex = p.get_contract_state(&addr).await.unwrap();
    assert!(hex.is_some(), "deployed contract should have state");
    eprintln!("contract state: {} hex chars", hex.unwrap().len());
}

#[tokio::test]
async fn get_contract_action() {
    let (p, addr) = require_contract!();
    let action = p.get_contract_action(&addr).await.unwrap();
    assert!(action.is_some());
    let action = action.unwrap();
    assert_eq!(action.address(), addr);
}

#[tokio::test]
#[ignore] // requires running forked devnet with midnight_queryContractState
async fn query_contract_state() {
    let provider = MidnightProvider::new("ws://127.0.0.1:9944", "http://127.0.0.1:8088").unwrap();

    let contract_address =
        "b5d05c8c96273a6c816d431648aba8355a8bf7c71df8a6b5d2fe0d6891b05dcf";

    // Test 1: Read counter (field 1, Cell)
    let results = provider
        .query_contract_state(
            contract_address,
            vec![StateQuery {
                field_path: vec![1],
                key: None,
            }],
        )
        .await
        .unwrap();
    assert_eq!(results.len(), 1);
    assert!(results[0].found);
    eprintln!("Counter value: {:?}", results[0].value);

    // Test 2: Get map size (field 0, no key)
    let results = provider
        .query_contract_state(
            contract_address,
            vec![StateQuery {
                field_path: vec![0],
                key: None,
            }],
        )
        .await
        .unwrap();
    assert!(results[0].found);
    // Value is u64 LE hex: "6400000000000000" = 100
    assert_eq!(results[0].value.as_deref(), Some("6400000000000000"));
    eprintln!("Map size: 100");

    // Test 3: Look up Fr(42) in map — key hex "412a41"
    let results = provider
        .query_contract_state(
            contract_address,
            vec![StateQuery {
                field_path: vec![0],
                key: Some("412a41".to_string()),
            }],
        )
        .await
        .unwrap();
    assert!(results[0].found);
    eprintln!("Fr(42) found: value = {:?}", results[0].value);

    // Test 4: Look up Fr(100) — should NOT exist
    let results = provider
        .query_contract_state(
            contract_address,
            vec![StateQuery {
                field_path: vec![0],
                key: Some("416441".to_string()),
            }],
        )
        .await
        .unwrap();
    assert!(!results[0].found);
    eprintln!("Fr(100) not found");

    // Test 5: Batch query — counter + 3 map keys
    let results = provider
        .query_contract_state(
            contract_address,
            vec![
                StateQuery {
                    field_path: vec![1],
                    key: None,
                },
                StateQuery {
                    field_path: vec![0],
                    key: Some("4041".to_string()),
                }, // Fr(0)
                StateQuery {
                    field_path: vec![0],
                    key: Some("0141".to_string()),
                }, // Fr(1)
                StateQuery {
                    field_path: vec![0],
                    key: Some("416341".to_string()),
                }, // Fr(99)
            ],
        )
        .await
        .unwrap();
    assert_eq!(results.len(), 4);
    assert!(results[0].found); // counter
    assert!(results[1].found); // Fr(0)
    assert!(results[2].found); // Fr(1)
    assert!(results[3].found); // Fr(99)
    eprintln!("Batch query: 4/4 found");
}
