//! Counter contract example — deploy to a dev node and interact.
//!
//! ```bash
//! docker compose -f devnet/docker-compose.yml up -d   # from the repo root
//! # wait for node RPC
//! while ! curl -sf http://localhost:9944/health > /dev/null 2>&1; do sleep 2; done
//! # wait for indexer (any HTTP response means the port is serving)
//! while ! curl -s --max-time 2 http://localhost:8088 > /dev/null 2>&1; do sleep 2; done
//! cargo run -p example-counter
//! docker compose -f devnet/docker-compose.yml down
//! ```

use midnight_provider::{MidnightProvider, Network, Seed};

mod counter {
    // Shared contract artifacts (see devnet/contracts/counter), reused by the
    // contract-maintenance example too.
    midnight_bindgen::contract!("../../devnet/contracts/counter/compiled/contract-info.json");
}

/// Node/indexer URLs default to the local devnet; override with the
/// `MIDNIGHT_NODE_URL` / `MIDNIGHT_INDEXER_URL` env vars to run elsewhere.
fn env_or(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_string())
}
const ZK_KEYS_DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../devnet/contracts/counter/compiled"
);

/// Dev node genesis wallet seed (funded with NIGHT tokens at genesis).
const DEV_WALLET_SEED: &str = "0000000000000000000000000000000000000000000000000000000000000001";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Midnight Counter Example ===\n");

    // The provider owns the URLs; sync_wallet drives the zswap + dust +
    // unshielded sync against the provider's indexer.
    println!("0. Syncing wallet state from indexer...");
    let seed = Seed::from_hex(DEV_WALLET_SEED)?;
    let node_url = env_or("MIDNIGHT_NODE_URL", "ws://127.0.0.1:9944");
    let indexer_url = env_or("MIDNIGHT_INDEXER_URL", "http://127.0.0.1:8088");
    let provider = MidnightProvider::new(&node_url, &indexer_url)?
        .sync_wallet(seed, Network::Undeployed)
        .await?;
    println!("   synced.\n");

    // 1. Deploy the contract; observe Best then Finalized inclusion.
    println!("1. Deploying counter contract...");
    let pending = counter::Contract::deploy(&provider)
        .with_initial_state(counter::LedgerInitialState::default())
        .with_zk_config(ZK_KEYS_DIR)
        .send()
        .await?;
    println!("   ext hash:  {}", pending.extrinsic_hash_hex());
    let (best, pending) = pending.wait_best().await?;
    println!("   best:      {}", hex::encode(best.block_hash));
    let (finalized, pending) = pending.wait_finalized().await?;
    println!("   finalized: {}", hex::encode(finalized.block_hash));
    let contract = pending.into_contract().await?;
    let contract_address = contract.address().to_string();
    println!("   address:   {contract_address}");
    println!("   round = {}", contract.ledger().await?.round()?);

    // 2. Call increment on-chain (returns the increment amount)
    println!("2. Calling increment on-chain...");
    let returned: u64 = contract.circuits().increment().await?;
    println!("   returned = {returned}");
    println!("   round = {}", contract.ledger().await?.round()?);

    // 3. Call increment_by with an argument (returns the amount)
    println!("3. Calling increment_by(5) on-chain...");
    let returned: u16 = contract.circuits().increment_by(5).await?;
    println!("   returned = {returned}");
    println!("   round = {}", contract.ledger().await?.round()?);

    // 4. Reconnect via Contract::at (no network calls) and call through the
    //    fresh handle. Mirrors what a second process would do given just the
    //    address.
    println!("4. Reconnecting via Contract::at and calling increment...");
    let reconnected = counter::Contract::at(&provider, &contract_address)
        .with_zk_config(ZK_KEYS_DIR)
        .build();
    let returned: u64 = reconnected.circuits().increment().await?;
    println!("   returned = {returned}");
    println!("   round = {}", reconnected.ledger().await?.round()?);

    // 5. Read through a Finalized pin: every ledger() call resolves the
    //    current finalized head, so the value can't be reorged away. The
    //    increment above was awaited to finality, so the finalized view
    //    already includes it.
    println!("5. Reading the ledger pinned at the finalized block...");
    let finalized_view = counter::Contract::at(&provider, &contract_address)
        .with_zk_config(ZK_KEYS_DIR)
        .at_block(midnight_bindgen::midnight_contract::BlockRef::Finalized)
        .build();
    println!("   round = {}", finalized_view.ledger().await?.round()?);

    println!("\n=== Done ===");
    Ok(())
}
