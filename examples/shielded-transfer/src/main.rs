//! Shielded transfer example: a single-wallet self-transfer of 1 unit of the
//! default shielded token id against a local devnet. See README.md for setup.
//!
//! Why local-only: the public preprod faucet only funds *unshielded* addresses,
//! and the SDK has no `unshielded → shielded` "shield" operation. The
//! local dev preset of the midnight-node image mints several shielded test
//! tokens to the hardcoded dev seed at genesis, which is what this example
//! spends. See `docs/tokens.md` for the asset model.

use std::env;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, bail};
use midnight_core::wallet::LocalWallet;
use midnight_core::{MidnightProvider, Network, Wallet};
use tracing_subscriber::EnvFilter;

/// Hardcoded dev seed, funded with shielded test tokens at genesis on the
/// local devnet. Do NOT use in production.
const DEV_SEED: &str = "0000000000000000000000000000000000000000000000000000000000000001";

/// The pause between two passes of the background sync. A short pause ends a
/// wait soon after the indexer serves the transaction.
const SYNC_PAUSE: Duration = Duration::from_secs(1);

/// How long the wallet waits to see the effect of a finalized transaction.
const EFFECT_TIMEOUT: Duration = Duration::from_secs(60);

fn required_env(name: &str) -> String {
    env::var(name).unwrap_or_else(|_| {
        eprintln!("error: {name} environment variable is required");
        std::process::exit(1);
    })
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::from_default_env()
                .add_directive("midnight_wallet=info".parse()?)
                .add_directive("midnight_indexer_client=info".parse()?),
        )
        .with_target(true)
        .init();

    let node_url = required_env("MIDNIGHT_NODE_URL");
    let indexer_url = required_env("MIDNIGHT_INDEXER_URL");
    let network: Network = env::var("MIDNIGHT_NETWORK")
        .unwrap_or_else(|_| "undeployed".into())
        .parse()?;

    println!("=== Midnight Shielded Transfer ===\n");
    println!("Network:           {network}");
    println!("Node:              {node_url}");
    println!("Indexer:           {indexer_url}");
    println!();

    println!("Syncing wallet from indexer (zswap + dust + unshielded in parallel)...");
    let provider = MidnightProvider::new(&node_url, &indexer_url);
    let wallet = Wallet::from_seed(DEV_SEED)?
        .with_network(&network)
        .sync(&provider)
        .await?;
    let address = wallet.shielded_address();
    let wallet = Arc::new(LocalWallet::new(wallet));
    let provider = Arc::new(provider.with_wallet(Arc::clone(&wallet)));
    println!("Sync complete.");
    println!("Shielded address:  {address}\n");

    // Pick a coin worth more than 1 unit, so the transfer has a change output:
    // the default coin selection spends the largest coin of the token. The
    // local dev preset funds the dev seed with several shielded test tokens.
    let balance = provider.balance().await?;
    let Some(coin) = balance.shielded.coins.iter().find(|c| c.value > 1).cloned() else {
        bail!("wallet has no shielded coin worth more than 1 unit. Is this a fresh local devnet?");
    };
    println!("--- Pre-transfer shielded balance ---");
    for c in &balance.shielded.coins {
        println!("  {c}");
    }
    println!();

    println!(
        "Building shielded self-transfer: 1 unit of token {} back to own address",
        coin.token_type_hex()
    );

    println!("Building + submitting...");
    let pending = provider
        .transfer_shielded(coin.token_type, 1, &address)
        .await?;
    println!("Submitted: ext hash {}", pending.extrinsic_hash_hex());
    let (best, pending) = pending.wait_best().await?;
    println!("Best:      {}", hex::encode(best.block_hash));
    let (finalized, _) = pending.wait_finalized().await?;
    println!("Finalized: {}\n", hex::encode(finalized.block_hash));

    // The self-transfer spends one coin and outputs 1 unit plus the change, so
    // the wallet holds one more coin once a pass sees it.
    println!("Waiting for the wallet to see the outputs...");
    let sync = wallet.sync_in_background(provider.clone(), SYNC_PAUSE);
    tokio::time::timeout(
        EFFECT_TIMEOUT,
        sync.wait_until(|b| b.shielded.total_count > balance.shielded.total_count),
    )
    .await
    .map_err(|elapsed| {
        sync.last_error()
            .map_or_else(|| elapsed.into(), anyhow::Error::from)
    })
    .context("the wallet did not see the outputs")??;
    let post = provider.balance().await?;
    println!("\n--- Post-transfer shielded balance ---");
    for c in &post.shielded.coins {
        println!("  {c}");
    }
    println!(
        "\n--- Dust (paid the fee) ---\nbalance: {} SPECK, spendable UTXOs: {}",
        post.dust.balance_speck, post.dust.spendable_utxos,
    );

    println!("\n=== Done ===");
    Ok(())
}
