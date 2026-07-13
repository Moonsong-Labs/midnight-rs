//! Integration tests against a running Midnight devnet.
//! Skipped unless MIDNIGHT_INDEXER_URL and MIDNIGHT_NODE_URL are set.

use midnight_provider::{MidnightProvider, NodeBlockHash, Provider};

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
async fn finalized_height_trails_the_best_head() {
    let p = require_provider!();
    let finalized = p.get_finalized_block_height().await.unwrap();
    // Read best after finalized: finalized(t1) <= best(t1) <= best(t2).
    let best = p.get_block_number().await.unwrap();
    assert!(finalized > 0);
    assert!(
        finalized <= best,
        "finalized {finalized} must not exceed best {best}"
    );
}

#[tokio::test]
async fn hash_by_height_is_unique_when_finalized_and_empty_past_the_chain() {
    let p = require_provider!();
    let finalized = p.get_finalized_block_height().await.unwrap();
    let hashes = p.get_block_hashes_by_height(finalized).await.unwrap();
    assert_eq!(
        hashes.len(),
        1,
        "a finalized height must resolve to exactly one hash"
    );
    assert!(
        p.get_block_hashes_by_height(finalized + 1_000_000)
            .await
            .unwrap()
            .is_empty(),
        "a height the chain has not reached must resolve to no hashes"
    );
}

#[tokio::test]
async fn header_round_trips_the_finalized_height_and_nulls_unknown_hashes() {
    let p = require_provider!();
    let finalized = p.get_finalized_block_height().await.unwrap();
    let hash = p.get_block_hashes_by_height(finalized).await.unwrap()[0];
    let header = p
        .get_block_header(hash)
        .await
        .unwrap()
        .expect("the finalized block must have a header");
    assert_eq!(
        header.number, finalized,
        "the header's number must round-trip the height its hash was resolved from"
    );
    assert!(
        p.get_block_header(NodeBlockHash::zero())
            .await
            .unwrap()
            .is_none(),
        "an unknown hash must resolve to no header"
    );
}

#[tokio::test]
async fn finalized_hash_pins_a_node_state_read() {
    let (p, addr) = require_contract!();
    let finalized = p.get_finalized_block_height().await.unwrap();
    let hash = p.get_block_hashes_by_height(finalized).await.unwrap()[0];
    // H256's Debug form is the full 0x hex string the node RPC expects.
    let state = p
        .get_state_from_node(&addr, Some(&format!("{hash:?}")))
        .await
        .unwrap();
    assert!(
        state.is_some(),
        "a deployed contract must have state at the finalized head"
    );
}

/// Restarts the devnet node container out from under a live provider and
/// asserts the same provider recovers without being rebuilt (the underlying
/// websocket auto-reconnects). Ignored because it disrupts the node other
/// tests talk to; run it alone:
///
/// ```sh
/// MIDNIGHT_NODE_CONTAINER=<name> cargo test -p midnight-provider --test devnet -- --ignored
/// ```
#[tokio::test]
#[ignore = "restarts the devnet node; run alone with MIDNIGHT_NODE_CONTAINER set"]
async fn survives_a_node_restart() {
    let p = require_provider!();
    let Ok(container) = std::env::var("MIDNIGHT_NODE_CONTAINER") else {
        eprintln!("skipping: MIDNIGHT_NODE_CONTAINER not set");
        return;
    };

    let before = p.get_finalized_block_height().await.unwrap();

    let status = std::process::Command::new("docker")
        .args(["restart", &container])
        .status()
        .expect("docker restart must be runnable");
    assert!(status.success(), "docker restart {container} failed");

    // The websocket died with the node. The same provider instance must serve
    // reads again once the node is back and has caught up to where it was.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    loop {
        if let Ok(after) = p.get_finalized_block_height().await {
            if after >= before {
                break;
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "provider must recover within 120s of the node restart"
        );
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
}
