//! Wallet sync example: sync a wallet against the local devnet, or another
//! ledger 9 network, display balances, and optionally register Dust or submit
//! a self-transfer. See README.md for usage.

use std::env;

use anyhow::Context;
use midnight_core::provider::{SPECKS_PER_DUST, TransactionRequest};
use midnight_core::wallet::NIGHT;
use midnight_core::{MidnightProvider, Network, Seed, SyncProgress, Wallet};
use tracing_subscriber::EnvFilter;

/// Hardcoded dev seed, funded at genesis on the local devnet. Do NOT use in
/// production.
const DEV_SEED: &str = "0000000000000000000000000000000000000000000000000000000000000001";

fn env_or(name: &str, default: &str) -> String {
    env::var(name).unwrap_or_else(|_| default.to_string())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::from_default_env()
                .add_directive("midnight_wallet=info".parse()?)
                .add_directive("midnight_indexer_client=debug".parse()?),
        )
        .with_target(true)
        .init();

    let node_url = env_or("MIDNIGHT_NODE_URL", "ws://127.0.0.1:9944");
    let indexer_url = env_or("MIDNIGHT_INDEXER_URL", "http://127.0.0.1:8088");
    let network: Network = env_or("MIDNIGHT_NETWORK", "undeployed").parse()?;
    let seed: Seed = env_or("MIDNIGHT_WALLET_SEED", DEV_SEED).parse()?;

    println!("=== Midnight Wallet Sync ===\n");
    println!("Network:             {network}");
    println!("Unshielded address:  {}", seed.unshielded_address(&network));
    println!("Shielded address:    {}", seed.shielded_address(&network));
    println!("Node:                {node_url}");
    println!("Indexer:             {indexer_url}");
    let storage_dir = Wallet::default_storage_dir();
    if let Some(ref dir) = storage_dir {
        println!("Storage:             {}", dir.display());
    }
    println!();

    println!("Syncing wallet state from indexer (zswap and unshielded, then dust)...");
    println!("Dust sync may take 30+ minutes from genesis. Progress is checkpointed to disk.\n");
    let provider = MidnightProvider::new(&node_url);
    let mut builder = Wallet::from_seed(seed.clone()).with_network(&network);
    if let Some(dir) = storage_dir.as_ref() {
        builder = builder.with_storage(dir);
    }
    let (mut rx, handle) = builder.sync_with_progress(&provider, &indexer_url).await?;

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

    let wallet = handle.await?;
    println!("\nSync complete.\n");
    let provider = provider.with_wallet(&wallet);

    let balance = wallet.balance().await;
    // Both balance entry types impl Display; NIGHT renders as "NIGHT: <val>"
    // on the unshielded side, everything else as "<hex8>…: <val>".
    println!("--- Balances ---");
    println!("Shielded coins:   {}", balance.shielded.total_count);
    for coin in &balance.shielded.coins {
        println!("  {coin}");
    }
    println!("Unshielded UTXOs: {}", balance.unshielded.len());
    for utxo in &balance.unshielded {
        println!("  {utxo}");
    }

    println!("\n--- Dust ---");
    println!("Spendable UTXOs: {}", balance.dust.spendable_utxos);
    println!(
        "Dust balance:    {} SPECK ({:.6} DUST)",
        balance.dust.balance_speck,
        balance.dust.balance_speck as f64 / SPECKS_PER_DUST as f64
    );

    if env::var("REGISTER_DUST").is_ok() {
        println!("\n--- Dust Registration ---");
        println!("Building + submitting dust registration transaction...");
        let pending = provider
            .send_transaction(TransactionRequest::register_dust())
            .await?;
        println!("Submitted! Tx hash: {:x}", pending.extrinsic_hash());
        pending.get_receipt().await?;
        println!("Finalized and applied.");
    }

    if let Ok(amount_str) = env::var("TRANSFER_AMOUNT") {
        let amount: u128 = amount_str.parse().with_context(|| {
            format!(
                "TRANSFER_AMOUNT must be a valid integer (atomic units / STAR), got {amount_str:?}"
            )
        })?;

        println!("\n--- Unshielded Self-Transfer ---");
        println!("Amount: {amount} STAR (atomic tNIGHT units)");
        println!("Recipient: {}", seed.unshielded_address(&network));
        println!("Building + submitting unshielded transfer (fees paid with real dust UTXOs)...");
        let pending = provider
            .send_transaction(TransactionRequest::unshielded(
                NIGHT,
                amount,
                seed.unshielded_recipient(),
            ))
            .await?;
        println!("Submitted! Tx hash: {:x}", pending.extrinsic_hash());
        pending.get_receipt().await?;
        println!("Finalized and applied.");
    }

    println!("\n=== Done ===");
    Ok(())
}
