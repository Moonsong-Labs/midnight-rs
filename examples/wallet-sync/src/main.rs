//! Wallet sync example — connect to any Midnight network, display balances, and
//! optionally register Dust or submit a self-transfer. See README.md for usage.

use std::env;

use midnight_provider::{MidnightProvider, Network, Seed, SyncProgress};
use midnight_wallet::{NIGHT, Wallet};
use tracing_subscriber::EnvFilter;

// Default seed for the preprod faucet flow. Override with `MIDNIGHT_WALLET_SEED`
// to point at a different wallet — e.g. the local dev devnet's prefunded seed
// (`0000…0001`) to see non-empty balances and dust generation.
//
// `MIDNIGHT_WALLET_SEED` is parsed through `Seed`'s `FromStr` impl, which
// accepts a 16/32/64-byte hex string or a BIP-39 mnemonic phrase (12/15/18/21/24
// words). For a mnemonic with a passphrase, use `Seed::from_mnemonic_with_passphrase`
// directly. Hard-coded here as the default for dev/example purposes only; do
// NOT use in production.
const DEFAULT_SEED: &str = "13e772040e60bf21946c1f15dbf8161cf4ff05266f62830437d5c1c7ec72480f";

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
    let network: Network = env::var("MIDNIGHT_NETWORK")
        .unwrap_or_else(|_| "preprod".into())
        .into();

    let seed_input = env::var("MIDNIGHT_WALLET_SEED").unwrap_or_else(|_| DEFAULT_SEED.into());
    // `Seed::FromStr` tries hex first, then BIP-39 mnemonic — drop in whichever
    // format you have. For mnemonic + passphrase use `Seed::from_mnemonic_with_passphrase`.
    let seed: Seed = seed_input.parse()?;

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

    println!("Syncing wallet state from indexer (zswap + unshielded + dust in parallel)...");
    println!("Dust sync may take 30+ minutes from genesis. Progress is checkpointed to disk.\n");
    let mut sync =
        MidnightProvider::new(&node_url, &indexer_url)?.sync_wallet(seed.clone(), &network);
    if let Some(dir) = storage_dir.as_ref() {
        sync = sync.with_storage(dir);
    }
    let (mut rx, handle) = sync.stream();

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

    let provider = handle.await?;
    println!("\nSync complete.\n");

    let balance = provider.balance().await?;
    // Both balance entry types impl Display; NIGHT renders as "NIGHT: <val>"
    // on the unshielded side, everything else as "<hex8>…: <val>".
    println!("--- Balances ---");
    println!("Shielded coins: {}", balance.shielded.total_count);
    for coin in &balance.shielded.coins {
        println!("  {coin}");
    }
    println!("Unshielded:     {} token type(s)", balance.unshielded.len());
    for utxo in &balance.unshielded {
        println!("  {utxo}");
    }

    {
        let wallet = provider.wallet().await?;
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
            .filter(|u| u.token_type == NIGHT)
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
        println!("Building + submitting dust registration transaction...");
        let pending = provider.register_dust(None).await?;
        println!("Submitted! Tx hash: {}", pending.extrinsic_hash_hex());
        let (_, _) = pending.wait_best().await?;
        println!("Included in best block.");
    }

    if let Ok(amount_str) = env::var("TRANSFER_AMOUNT") {
        let amount: u128 = amount_str.parse().map_err(|e| {
            format!("TRANSFER_AMOUNT must be a valid integer (atomic units / STAR): {e}")
        })?;

        if !provider.dust_synced().await? {
            return Err("Dust sync required for transfers. Run a full sync first.".into());
        }

        let recipient = seed.unshielded_address(&network);
        println!("\n--- Unshielded Self-Transfer ---");
        println!("Amount: {amount} STAR (atomic tNIGHT units)");
        println!("Recipient: {recipient}");
        println!("Building + submitting unshielded transfer (fees paid with real dust UTXOs)...");
        let pending = provider
            .transfer_unshielded(NIGHT, amount, &recipient)
            .await?;
        println!("Submitted! Tx hash: {}", pending.extrinsic_hash_hex());
        let (_, _) = pending.wait_best().await?;
        println!("Included in best block.");
    }

    println!("\n=== Done ===");
    Ok(())
}
