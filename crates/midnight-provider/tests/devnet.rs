//! Integration tests against a running Midnight devnet.
//! Skipped unless MIDNIGHT_INDEXER_URL and MIDNIGHT_NODE_URL are set.

use midnight_provider::{MidnightProvider, Provider};

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
    let block = p.get_block(None).await.unwrap().unwrap();
    assert!(block.height > 0);
}

#[tokio::test]
async fn get_contract_state() {
    let (p, addr) = require_contract!();
    let hex = p.get_contract_state(&addr, None).await.unwrap();
    assert!(hex.is_some(), "deployed contract should have state");
    eprintln!("contract state: {} hex chars", hex.unwrap().len());
}

#[tokio::test]
async fn get_contract_action() {
    let (p, addr) = require_contract!();
    let action = p.get_contract_action(&addr, None).await.unwrap();
    assert!(action.is_some());
    let action = action.unwrap();
    assert_eq!(action.address(), addr);
}

#[tokio::test]
async fn get_block_hash_resolves_existing_heights_and_nulls_future_ones() {
    let p = require_provider!();
    let height = p.get_block_number().await.unwrap();
    let hash = p.get_block_hash(height as u64).await.unwrap();
    assert!(
        hash.is_some_and(|h| h.starts_with("0x")),
        "an existing height must resolve to a 0x hash"
    );
    assert_eq!(
        p.get_block_hash(height as u64 + 1_000_000).await.unwrap(),
        None,
        "a height the chain has not reached must resolve to None"
    );
}

#[tokio::test]
async fn finalized_block_number_trails_the_best_head() {
    let p = require_provider!();
    let finalized = p.get_finalized_block_number().await.unwrap();
    // Read best after finalized: finalized(t1) <= best(t1) <= best(t2).
    let best = p.get_block_number().await.unwrap();
    assert!(finalized > 0);
    assert!(
        finalized <= best,
        "finalized {finalized} must not exceed best {best}"
    );
}

#[tokio::test]
async fn finalized_block_hash_pins_a_node_state_read() {
    let (p, addr) = require_contract!();
    let hash = p.get_finalized_block_hash().await.unwrap();
    assert!(hash.starts_with("0x"));
    let state = p.get_state_from_node(&addr, Some(&hash)).await.unwrap();
    assert!(
        state.is_some(),
        "a deployed contract must have state at the finalized head"
    );
}
