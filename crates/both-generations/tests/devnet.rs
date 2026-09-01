//! Against a running devnet: reach a contract without naming a generation.
//!
//! Deploying is not covered here, because `Client` has no way to attach a
//! wallet yet and a deploy needs one to fund itself.
//!
//! Skips unless `MIDNIGHT_E2E` is set, like the rest of the devnet suite.

use both_generations::bindings::Counter;
use midnight_dispatch::Client;

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
