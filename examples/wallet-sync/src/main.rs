//! Wallet sync example — connect to any Midnight network, display balances, and
//! optionally register Dust or submit a self-transfer. See README.md for usage.

use std::env;

use midnight_provider::{MidnightProvider, SyncProgress, WalletSeed};
use midnight_wallet::{Wallet, address};
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
        .sync_wallet_with_progress(seed.clone(), &network, storage_dir.as_deref());

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
    // Unshielded label: the zero token id is NIGHT (the chain's native
    // unshielded token). For other unshielded tokens (contract-minted, etc.)
    // show a hex prefix.
    let unshielded_label = |token: &str| -> String {
        if token == night_hex {
            "tNIGHT".into()
        } else {
            format!("{}...", &token[..8])
        }
    };
    // Shielded coin label: the zero token id is *not* NIGHT (there is no
    // shielded NIGHT — see docs/tokens.md). Treat shielded token ids as
    // opaque; show a hex prefix in all cases.
    let shielded_label = |token: &str| -> String { format!("{}...", &token[..8]) };

    println!("--- Balances ---");
    println!("Shielded coins: {}", balance.shielded.total_count);
    for coin in &balance.shielded.coins {
        println!("  {}: {}", shielded_label(&coin.token_type), coin.value);
    }
    println!("Unshielded:     {} token type(s)", balance.unshielded.len());
    for utxo in &balance.unshielded {
        println!("  {}: {}", unshielded_label(&utxo.token_type), utxo.value);
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
        println!("Building dust registration transaction...");
        let result = provider.register_dust(None).await?;

        println!("Submitting to node...");
        let pending = provider.submit(&result.tx_bytes).await?;
        println!("Submitted! Tx hash: {}", pending.extrinsic_hash_hex());
        let (_, _) = pending.wait_best().await?;
        println!("Included in best block.");
    }

    if let Ok(amount_str) = env::var("TRANSFER_AMOUNT") {
        let amount: u128 = amount_str.parse().map_err(|e| {
            format!("TRANSFER_AMOUNT must be a valid integer (atomic units / STAR): {e}")
        })?;

        if !provider.dust_synced().await {
            return Err("Dust sync required for transfers. Run a full sync first.".into());
        }

        let recipient = address::derive_unshielded(&seed, &network);
        println!("\n--- Unshielded Self-Transfer ---");
        println!("Amount: {amount} STAR (atomic tNIGHT units)");
        println!("Recipient: {recipient}");
        println!("Building unshielded transfer (fees paid with real dust UTXOs)...");
        let result = provider
            .transfer_unshielded(midnight_wallet::NIGHT, amount, &recipient)
            .await?;

        println!("Submitting to node...");
        let pending = provider.submit(&result.tx_bytes).await?;
        println!("Submitted! Tx hash: {}", pending.extrinsic_hash_hex());
        let (_, _) = pending.wait_best().await?;
        println!("Included in best block.");
    }

    println!("\n=== Done ===");
    Ok(())
}
