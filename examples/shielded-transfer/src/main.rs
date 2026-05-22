//! Shielded transfer example — single-wallet self-transfer of 1 unit of the
//! default shielded token id against a local devnet. See README.md for setup.
//!
//! Why local-only: the public preprod faucet only funds *unshielded* addresses,
//! and the SDK has no `unshielded → shielded` "shield" operation today. The
//! local dev preset of the midnight-node image mints several shielded test
//! tokens to the hardcoded dev seed at genesis, which is what this example
//! spends. See `docs/tokens.md` for the asset model.

use std::env;

use midnight_provider::{MidnightProvider, WalletSeed};
use midnight_wallet::address;
use tracing_subscriber::EnvFilter;

/// Hardcoded dev seed, funded with shielded test tokens at genesis on the
/// local devnet. Do NOT use in production.
const DEV_SEED: &str = "0000000000000000000000000000000000000000000000000000000000000001";

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
                .add_directive("midnight_indexer_client=info".parse()?),
        )
        .with_target(true)
        .init();

    let node_url = required_env("MIDNIGHT_NODE_URL");
    let indexer_url = required_env("MIDNIGHT_INDEXER_URL");
    let network = env::var("MIDNIGHT_NETWORK").unwrap_or_else(|_| "undeployed".into());

    let seed = WalletSeed::try_from_hex_str(DEV_SEED)?;

    println!("=== Midnight Shielded Transfer ===\n");
    println!("Network:           {network}");
    println!(
        "Shielded address:  {}",
        address::derive_shielded(&seed, &network)
    );
    println!("Node:              {node_url}");
    println!("Indexer:           {indexer_url}");
    println!();

    println!("Syncing wallet from indexer (zswap + dust + unshielded in parallel)...");
    let provider = MidnightProvider::new(&node_url, &indexer_url)?
        .sync_wallet(seed.clone(), &network, None)
        .await?;
    println!("Sync complete.\n");

    // No manual chain-readiness wait needed — every transfer / contract
    // path goes through MidnightProvider::resync_wallet, which now waits
    // internally for the chain to advance past the dev-devnet's hardcoded
    // genesis block (a no-op on any chain with block height ≥ 1).

    // Pick the first shielded coin in the wallet. The local dev preset funds
    // the dev seed with several shielded test tokens; on a fresh devnet the
    // default-id token ([0; 32]) is always there.
    let balance = provider.balance().await.expect("wallet attached");
    let Some(coin) = balance.shielded.coins.first().cloned() else {
        return Err("wallet has no shielded coins to spend — is this a fresh local devnet?".into());
    };
    let coin_hex = coin.token_type_hex();
    println!("--- Pre-transfer shielded balance ---");
    for c in &balance.shielded.coins {
        let hex = c.token_type_hex();
        println!("  ...{}: {}", &hex[hex.len() - 8..], c.value);
    }
    println!();

    println!(
        "Building shielded self-transfer: 1 unit of token ...{} back to own address",
        &coin_hex[coin_hex.len() - 8..]
    );
    let recipient = address::derive_shielded(&seed, &network);

    let result = provider
        .transfer_shielded(coin.token_type, 1, &recipient)
        .await?;
    println!("Built: tx_bytes={}\n", result.tx_bytes.len());

    println!("Submitting...");
    let pending = provider.submit(&result.tx_bytes).await?;
    println!("Submitted: ext hash {}", pending.extrinsic_hash_hex());
    let (best, pending) = pending.wait_best().await?;
    println!("Best:      {}", hex::encode(best.block_hash));
    let (finalized, _) = pending.wait_finalized().await?;
    println!("Finalized: {}\n", hex::encode(finalized.block_hash));

    println!("Resyncing...");
    provider.resync_wallet().await?;
    let post = provider.balance().await.expect("wallet attached");
    println!("\n--- Post-transfer shielded balance ---");
    for c in &post.shielded.coins {
        let hex = c.token_type_hex();
        println!("  ...{}: {}", &hex[hex.len() - 8..], c.value);
    }
    println!(
        "\n--- Dust (paid the fee) ---\nbalance: {} SPECK, spendable UTXOs: {}",
        post.dust.balance_speck, post.dust.spendable_utxos,
    );

    println!("\n=== Done ===");
    Ok(())
}
