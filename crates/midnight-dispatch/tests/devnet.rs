//! Against a running devnet: does the client pick the generation the chain runs?
//!
//! Skips unless `MIDNIGHT_E2E` is set, like the rest of the devnet suite.

use midnight_dispatch::{Client, Generation};

fn devnet() -> Option<(String, String)> {
    if std::env::var("MIDNIGHT_E2E").is_err() {
        return None;
    }
    Some((
        std::env::var("MIDNIGHT_NODE_URL").ok()?,
        std::env::var("MIDNIGHT_INDEXER_URL").ok()?,
    ))
}

/// The devnet runs ledger 9 and reports it as `=1.0.0`, which is the case a
/// version-to-generation rule gets wrong: the major number is 1, not 9.
#[tokio::test]
async fn picks_the_generation_the_devnet_runs() {
    let Some((node, indexer)) = devnet() else {
        eprintln!("skipped: set MIDNIGHT_E2E");
        return;
    };
    let client = Client::connect(&node, &indexer)
        .await
        .expect("connect to the devnet");

    let reported = client.ledger_version().await.expect("ledger version");
    eprintln!("node reports {reported:?}, client speaks {}", client.generation());
    assert_eq!(client.generation(), Generation::Ledger9);

    let health = client.health().await.expect("health");
    assert!(health.node_connected, "the node answered the version query");
}
