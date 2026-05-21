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

use midnight_provider::{MidnightProvider, SyncProgress, WalletSeed};
use midnight_wallet::{LocalProofServer, Wallet, address};
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

    let seed = WalletSeed::try_from_hex_str(EXAMPLE_SEED)?;

    println!("=== Midnight Wallet Sync ===\n");
    println!("Network:             {network}");
    println!(
        "Unshielded address:  {}",
        address::derive_unshielded(&seed, &network)
    );
    println!(
        "Shielded address:    {}",
        address::derive_shielded(&seed, &network)
    );
    println!("Node:                {node_url}");
    println!("Indexer:             {indexer_url}");
    let storage_dir = Wallet::default_storage_dir();
    if let Some(ref dir) = storage_dir {
        println!("Storage:             {}", dir.display());
    }
    println!();

    println!("Syncing wallet state from indexer (zswap + unshielded + dust in parallel)...");
    println!("Dust sync may take 30+ minutes from genesis. Progress is checkpointed to disk.\n");
    let (mut rx, handle) = MidnightProvider::new(&node_url, &indexer_url)?
        .sync_wallet_with_progress(seed, &network, storage_dir.as_deref());

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

    let provider = handle.await??;
    println!("\nSync complete.\n");

    let balance = provider.balance().await.expect("wallet attached");
    let night_hex = "0".repeat(64);
    let label = |token: &str| -> String {
        if token == night_hex {
            "tNIGHT".into()
        } else {
            format!("{}...", &token[..8])
        }
    };

    println!("--- Balances ---");
    println!("Shielded coins: {}", balance.shielded.total_count);
    for coin in &balance.shielded.coins {
        println!("  {}: {}", label(&coin.token_type), coin.value);
    }
    println!("Unshielded:     {} token type(s)", balance.unshielded.len());
    for utxo in &balance.unshielded {
        println!("  {}: {}", label(&utxo.token_type), utxo.value);
    }

    {
        let wallet = provider.wallet_read().await.expect("wallet attached");
        println!("\n--- Dust ---");
        let dust_params = &wallet.parameters().dust;
        println!(
            "Dust ratio:      {} DUST/NIGHT",
            dust_params.night_dust_ratio
        );
        println!(
            "Decay rate:      {} SPECK/STAR/sec",
            dust_params.generation_decay_rate
        );
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
        println!("Zswap event ID:  {}", wallet.zswap_event_id());
        println!("Dust event ID:   {}", wallet.dust_event_id());
        println!("Last block:      {}", wallet.last_block_height());
        println!("Last tx ID:      {:?}", wallet.last_tx_id());
    }

    if env::var("REGISTER_DUST").is_ok() {
        println!("\n--- Dust Registration ---");

        let context = provider.build_context().await?;
        let proof_provider = Arc::new(LocalProofServer::new());
        let wallet_arc = provider.wallet().expect("wallet attached");
        let wallet = wallet_arc.read().await;
        let transfer = midnight_wallet::TransferBuilder::new(&wallet, context, proof_provider);

        println!("Building dust registration transaction...");
        let result = transfer.register_dust(None).await?;
        drop(wallet);

        println!("Submitting to node...");
        let hash = result.submit(&node_url).await?;
        println!("Submitted! Tx hash: {hash}");
    }

    if let Ok(amount_str) = env::var("TRANSFER_AMOUNT") {
        let amount: u128 = amount_str.parse().map_err(|e| {
            format!("TRANSFER_AMOUNT must be a valid integer (atomic units / STAR): {e}")
        })?;

        {
            let wallet = provider.wallet_read().await.expect("wallet attached");
            if !wallet.dust_synced() {
                return Err("Dust sync required for transfers. Run a full sync first.".into());
            }
        }

        println!("\n--- Unshielded Self-Transfer ---");
        println!("Amount: {amount} STAR (atomic tNIGHT units)");

        let context = provider.build_context().await?;
        let proof_provider = Arc::new(LocalProofServer::new());
        let wallet_arc = provider.wallet().expect("wallet attached");

        let token_type = midnight_wallet::NIGHT;
        let result;
        let to_seed;
        {
            let wallet = wallet_arc.read().await;
            to_seed = *wallet.seed();
            let transfer =
                midnight_wallet::TransferBuilder::new(&wallet, context.clone(), proof_provider);
            println!("Building unshielded transfer (fees paid with real dust UTXOs)...");
            result = transfer.unshielded(token_type, amount, to_seed).await?;
        }

        {
            let mut wallet = wallet_arc.write().await;
            wallet.sync_dust_from_context(&context);
            wallet.remove_unshielded_spent(&result.spent_unshielded_inputs);
            if let Some(ref dir) = storage_dir {
                wallet.save(dir)?;
            }
        }

        println!("Submitting to node...");
        let hash = result.submit(&node_url).await?;
        println!("Submitted! Tx hash: {hash}");
    }

    println!("\n=== Done ===");
    Ok(())
}
