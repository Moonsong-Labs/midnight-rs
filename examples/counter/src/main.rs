//! Counter contract example — deploy to a dev node and interact.
//!
//! ```bash
//! cd examples/counter && docker compose up -d
//! while ! curl -sf http://localhost:9944/health > /dev/null 2>&1; do sleep 2; done
//! cargo run -p example-counter
//! docker compose down
//! ```

use midnight_contract::interpreter::Value;
use midnight_provider::MidnightProvider;

mod counter {
    midnight_bindgen::contract!("compiled/contract-info.json");
}

const NODE_URL: &str = "ws://127.0.0.1:9944";
const INDEXER_URL: &str = "http://127.0.0.1:8088";
const ZK_KEYS_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/compiled");

/// Dev node genesis wallet seed (funded with NIGHT tokens at genesis).
const DEV_WALLET_SEED: &str = "0000000000000000000000000000000000000000000000000000000000000001";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Midnight Counter Example ===\n");

    let provider = MidnightProvider::new(NODE_URL, INDEXER_URL)?.with_wallet(DEV_WALLET_SEED);

    // 1. Deploy the contract
    println!("1. Deploying counter contract...");
    let mut contract = counter::Contract::deploy()
        .provider(&provider)
        .initial_state(counter::LedgerInitialState { round: 0 })
        .zk_keys(ZK_KEYS_DIR)
        .deploy()
        .await?;
    let address = contract.address().to_string();
    println!("   Deployed at: {address}");
    println!("   round = {}", contract.ledger().round()?);

    // 2. Call increment on-chain
    println!("2. Calling increment on-chain...");
    contract.circuits().increment().await?;
    println!("   round = {}", contract.ledger().round()?);

    // 3. Connect to the same contract from scratch and call increment again.
    //    Wait for the indexer to catch up with the latest on-chain state
    //    before reconnecting, otherwise we'd fetch stale state.
    println!("3. Waiting for indexer to sync, then reconnecting...");
    tokio::time::sleep(std::time::Duration::from_secs(30)).await;
    let mut contract = counter::Contract::connect(&address, &provider)
        .await?
        .with_zk_keys(ZK_KEYS_DIR);
    println!("   round = {}", contract.ledger().round()?);

    // 4. Call increment_by with an argument
    println!("4. Calling increment_by(5) on-chain...");
    contract.circuits().increment_by(Value::Integer(5)).await?;
    println!("   round = {}", contract.ledger().round()?);

    println!("\n=== Done ===");
    Ok(())
}
