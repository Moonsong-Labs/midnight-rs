//! Wallet sync example — connect to any Midnight network and display balances.
//!
//! Uses a hard-coded seed for deterministic addresses across runs.
//! Fund the unshielded address via the faucet before running.
//!
//! # Preprod
//!
//! 1. Fund `mn_addr_preprod1cu74c4snt48ztvvjfhlgjx64ydqy25y682ujtjde034l36umcxfsg697rj`
//!    via https://faucet.preprod.midnight.network/
//!
//! 2. Run:
//!    ```bash
//!    MIDNIGHT_NODE_URL="wss://rpc.preprod.midnight.network" \
//!    MIDNIGHT_INDEXER_URL="https://indexer.preprod.midnight.network" \
//!    MIDNIGHT_NETWORK="preprod" \
//!      cargo run --release -p example-wallet-sync
//!    ```
//!
//! # Devnet (local)
//!
//! ```bash
//! cd examples/counter && docker compose up -d
//! MIDNIGHT_NODE_URL="ws://127.0.0.1:9944" \
//! MIDNIGHT_INDEXER_URL="http://127.0.0.1:8088" \
//! MIDNIGHT_NETWORK="undeployed" \
//!   cargo run -p example-wallet-sync
//! ```

use std::env;

use midnight_wallet::{Wallet, WalletState};
use tracing_subscriber::EnvFilter;

// Intentionally hard-coded for dev/example purposes only. Do NOT use in production.
const EXAMPLE_SEED: &str = "13e772040e60bf21946c1f15dbf8161cf4ff05266f62830437d5c1c7ec72480f";

fn required_env(name: &str) -> String {
    env::var(name).unwrap_or_else(|_| {
        eprintln!("error: {name} environment variable is required");
        std::process::exit(1);
    })
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::from_default_env().add_directive("midnight_wallet=info".parse()?),
        )
        .with_target(false)
        .init();

    let node_url = required_env("MIDNIGHT_NODE_URL");
    let indexer_url = required_env("MIDNIGHT_INDEXER_URL");
    let network = env::var("MIDNIGHT_NETWORK").unwrap_or_else(|_| "preprod".into());

    let wallet = Wallet::from_seed_hex(EXAMPLE_SEED, &network)?;

    println!("=== Midnight Wallet Sync ===\n");
    println!("Network:             {network}");
    println!("Unshielded address:  {}", wallet.unshielded_address());
    println!("Shielded address:    {}", wallet.shielded_address());
    println!("Node:                {node_url}");
    println!("Indexer:             {indexer_url}");
    println!();

    println!("Syncing wallet state from indexer...");
    let state = WalletState::sync_from_indexer(
        &node_url,
        &indexer_url,
        *wallet.seed(),
        &wallet.unshielded_address(),
        &network,
    )
    .await?;

    println!("Sync complete.\n");

    let balance = state.balance();

    println!("--- Balances ---");
    println!("Dust UTXOs:     {}", balance.dust.spendable_utxos);
    println!("Shielded coins: {}", balance.shielded.total_count);
    for coin in &balance.shielded.coins {
        let token_label = if coin.token_type == "0".repeat(64) {
            "tNIGHT".to_string()
        } else {
            format!("{}...", &coin.token_type[..8])
        };
        println!("  {token_label}: {}", coin.value);
    }
    println!("Unshielded:     {} token type(s)", balance.unshielded.len());
    for utxo in &balance.unshielded {
        let token_label = if utxo.token_type == "0".repeat(64) {
            "tNIGHT".to_string()
        } else {
            format!("{}...", &utxo.token_type[..8])
        };
        println!("  {token_label}: {}", utxo.value);
    }

    println!("\n--- Sync state ---");
    println!("Zswap event ID:  {}", state.zswap_event_id());
    println!("Dust event ID:   {}", state.dust_event_id());
    println!("Last block:      {}", state.last_block_height());
    println!("Last tx ID:      {:?}", state.last_tx_id());

    println!("\n=== Done ===");
    Ok(())
}
