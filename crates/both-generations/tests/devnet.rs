//! Against a running devnet: reach a contract without naming a generation.
//!
//! Deploying is covered too: the client attaches a wallet from a seed, and
//! the generation is never named.
//!
//! Skips unless `MIDNIGHT_E2E` is set, like the rest of the devnet suite.

use both_generations::bindings::Counter;
use midnight_dispatch::{Client, Opening, OpeningField};

#[tokio::test]
async fn connects_and_reads_without_naming_a_generation() {
    if std::env::var("MIDNIGHT_E2E").is_err() {
        eprintln!("skipped: set MIDNIGHT_E2E");
        return;
    }
    let node = std::env::var("MIDNIGHT_NODE_URL").expect("MIDNIGHT_NODE_URL");
    let indexer = std::env::var("MIDNIGHT_INDEXER_URL").expect("MIDNIGHT_INDEXER_URL");

    // Nothing below names a ledger generation.
    let client = Client::connect(&node, &indexer).await.expect("connect");
    let generation = client.generation();
    eprintln!("the chain runs {generation}");

    let state = client
        .contract_state("0200aa1d5e0a90b2a0e1c6bbd0b1e8f4a0d5c3b2a1908f7e6d5c4b3a2918071620")
        .await
        .expect("state query answered");

    // The address is a placeholder, so the indexer reports no such contract.
    // What matters is that the query dispatched and the reader parses for
    // whichever generation the chain runs.
    match state {
        Some(hex) => {
            let counter = Counter::from_hex(generation, &hex).expect("parse");
            eprintln!("round = {:?}", counter.round());
        }
        None => eprintln!("no contract at the placeholder address, as expected"),
    }
}

/// Deploy a counter and read its opening value back, naming no generation.
#[tokio::test]
async fn deploys_and_reads_back_without_naming_a_generation() {
    if std::env::var("MIDNIGHT_E2E").is_err() {
        eprintln!("skipped: set MIDNIGHT_E2E");
        return;
    }
    let node = std::env::var("MIDNIGHT_NODE_URL").expect("MIDNIGHT_NODE_URL");
    let indexer = std::env::var("MIDNIGHT_INDEXER_URL").expect("MIDNIGHT_INDEXER_URL");
    let mut seed = [0u8; 32];
    seed[31] = 1;

    let client = Client::connect(&node, &indexer)
        .await
        .expect("connect")
        .with_wallet(&indexer, seed, "undeployed")
        .await
        .expect("attach a wallet");
    let generation = client.generation();

    // Relative to this crate, not the workspace root: a test runs with its
    // own manifest directory as the working directory.
    let compiled = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../devnet/contracts/counter/compiled");
    let address = client
        .deploy(
            compiled.to_str().expect("utf-8 path"),
            Opening::new(vec![OpeningField::Counter(0)]),
        )
        .await
        .expect("deploy");
    eprintln!("deployed to {address} on {generation}");

    let hex = client
        .contract_state(&address)
        .await
        .expect("state query")
        .expect("the contract exists");
    let counter = Counter::from_hex(generation, &hex).expect("parse state");
    assert_eq!(
        counter.round().expect("round"),
        0,
        "a fresh counter opens at 0"
    );
}
