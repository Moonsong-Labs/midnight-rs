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
//! 2. Run (balance only):
//!    ```bash
//!    MIDNIGHT_NODE_URL="wss://rpc.preprod.midnight.network" \
//!    MIDNIGHT_INDEXER_URL="https://indexer.preprod.midnight.network" \
//!    MIDNIGHT_NETWORK="preprod" \
//!      cargo run --release -p example-wallet-sync
//!    ```
//!
//! 3. Run with dust registration (submits a transaction):
//!    ```bash
//!    MIDNIGHT_NODE_URL="wss://rpc.preprod.midnight.network" \
//!    MIDNIGHT_INDEXER_URL="https://indexer.preprod.midnight.network" \
//!    MIDNIGHT_NETWORK="preprod" \
//!    REGISTER_DUST=1 \
//!      cargo run --release -p example-wallet-sync
//!    ```
//!
//! 4. Run with unshielded self-transfer (pays fees with real dust):
//!    ```bash
//!    MIDNIGHT_NODE_URL="wss://rpc.preprod.midnight.network" \
//!    MIDNIGHT_INDEXER_URL="https://indexer.preprod.midnight.network" \
//!    MIDNIGHT_NETWORK="preprod" \
//!    TRANSFER_AMOUNT=100 \
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
use std::sync::Arc;

use midnight_wallet::{LocalProofServer, SyncProgress, Wallet, WalletState};
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
            EnvFilter::from_default_env()
                .add_directive("midnight_wallet=info".parse()?)
                .add_directive("midnight_indexer_client=debug".parse()?),
        )
        .with_target(true)
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
    let storage_dir = WalletState::default_storage_dir();
    if let Some(ref dir) = storage_dir {
        println!("Storage:             {}", dir.display());
    }
    println!();

    println!("Syncing wallet state from indexer (zswap + unshielded + dust in parallel)...");
    println!("Dust sync may take 30+ minutes from genesis. Progress is checkpointed to disk.\n");
    let (mut rx, handle) = WalletState::sync_with_progress(
        &node_url,
        &indexer_url,
        *wallet.seed(),
        &wallet.unshielded_address(),
        &network,
        storage_dir.as_deref(),
    )
    .await;

    while let Some(progress) = rx.recv().await {
        match progress {
            SyncProgress::Resuming {
                zswap_event_id,
                dust_event_id,
            } => {
                println!("  [resume]      zswap={zswap_event_id} dust={dust_event_id}");
            }
            SyncProgress::ZswapEvents { current, max } => {
                let pct = if max > 0 {
                    current as f64 / max as f64 * 100.0
                } else {
                    0.0
                };
                println!("  [zswap]       {current}/{max} ({pct:.1}%)");
            }
            SyncProgress::ZswapComplete { events } => {
                println!("  [zswap]       complete ({events} events)");
            }
            SyncProgress::DustEvents { current, max } => {
                let pct = if max > 0 {
                    current as f64 / max as f64 * 100.0
                } else {
                    0.0
                };
                println!("  [dust]        {current}/{max} ({pct:.1}%)");
            }
            SyncProgress::DustComplete { events } => {
                println!("  [dust]        complete ({events} events)");
            }
            SyncProgress::UnshieldedCaughtUp { utxos } => {
                println!("  [unshielded]  caught up ({utxos} UTXOs)");
            }
        }
    }

    let mut state = handle.await??;
    println!("\nSync complete.\n");

    let balance = state.balance();

    println!("--- Balances ---");
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

    println!("\n--- Dust ---");
    let dust_params = &state.parameters().dust;
    println!(
        "Dust ratio:      {} DUST/NIGHT",
        dust_params.night_dust_ratio
    );
    println!(
        "Decay rate:      {} SPECK/STAR/sec",
        dust_params.generation_decay_rate
    );
    let night_hex = "0".repeat(64);
    let night_value: u128 = balance
        .unshielded
        .iter()
        .filter(|u| u.token_type == night_hex)
        .map(|u| u.value)
        .sum();
    let max_dust = night_value.saturating_mul(dust_params.night_dust_ratio as u128);
    let rate = night_value.saturating_mul(dust_params.generation_decay_rate as u128);
    let time_to_cap = max_dust.checked_div(rate).unwrap_or(0);
    println!(
        "Max capacity:    {} SPECK ({:.6} DUST)",
        max_dust,
        max_dust as f64 / 1e15
    );
    println!(
        "Generation rate: {} SPECK/sec ({:.6} DUST/sec)",
        rate,
        rate as f64 / 1e15
    );
    println!(
        "Time to cap:     {} seconds ({:.1} days)",
        time_to_cap,
        time_to_cap as f64 / 86400.0
    );
    println!("Spendable UTXOs: {}", balance.dust.spendable_utxos);
    println!(
        "Dust balance:    {} SPECK ({:.6} DUST)",
        balance.dust.balance_speck,
        balance.dust.balance_speck as f64 / 1e15
    );

    println!("\n--- Sync state ---");
    println!("Zswap event ID:  {}", state.zswap_event_id());
    println!("Dust event ID:   {}", state.dust_event_id());
    println!("Last block:      {}", state.last_block_height());
    println!("Last tx ID:      {:?}", state.last_tx_id());

    if env::var("REGISTER_DUST").is_ok() {
        println!("\n--- Dust Registration ---");

        let context = state.build_context()?;
        let proof_provider = Arc::new(LocalProofServer::new());
        let transfer = midnight_wallet::TransferBuilder::new(&state, context, proof_provider);

        println!("Building dust registration transaction...");
        let result = transfer.register_dust(None).await?;

        println!("Submitting to node...");
        let hash = result.submit(&node_url).await?;
        println!("Submitted! Tx hash: {hash}");
    }

    if let Ok(amount_str) = env::var("TRANSFER_AMOUNT") {
        let amount: u128 = amount_str.parse().map_err(|e| {
            format!("TRANSFER_AMOUNT must be a valid integer (atomic units / STAR): {e}")
        })?;

        if !state.dust_synced() {
            return Err("Dust sync required for transfers. Run a full sync first.".into());
        }

        println!("\n--- Unshielded Self-Transfer ---");
        println!("Amount: {amount} STAR (atomic tNIGHT units)");

        let context = state.build_context()?;
        let proof_provider = Arc::new(LocalProofServer::new());
        let transfer =
            midnight_wallet::TransferBuilder::new(&state, context.clone(), proof_provider);

        let to_seed = *wallet.seed();
        let token_type = midnight_wallet::NIGHT;

        println!("Building unshielded transfer (fees paid with real dust UTXOs)...");
        let result = transfer.unshielded(token_type, amount, to_seed).await?;

        state.sync_dust_from_context(&context);
        state.remove_unshielded_spent(&result.spent_unshielded_inputs);
        if let Some(ref dir) = storage_dir {
            state.save(dir)?;
        }

        println!("Submitting to node...");
        let hash = result.submit(&node_url).await?;
        println!("Submitted! Tx hash: {hash}");
    }

    println!("\n=== Done ===");
    Ok(())
}
