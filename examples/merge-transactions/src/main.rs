//! Merge transactions example — atomically combine two proven transactions into
//! one and submit it, using build-only builds plus
//! [`MidnightProvider::merge_transactions`].
//!
//! This is the multi-party building block: each transaction is built and proved
//! separately, then merged and submitted so they apply atomically (all or
//! nothing). Here both come from a single wallet to keep the example
//! self-contained, a counter `increment` call and a shielded self-transfer, but
//! in a real multi-party flow the second transaction is one another party (a
//! solver / counterparty) already built and proved with its own keys and handed
//! you as bytes.
//!
//! The two new pieces this demonstrates:
//! - `contract.circuits().build_<circuit>()` — build and prove a contract call
//!   and return its bytes *without* submitting (the build-only mirror of the
//!   submitting `<circuit>()` methods).
//! - `provider.merge_transactions(&[..])` — combine proven transactions into one.
//!
//! ```bash
//! docker compose -f devnet/docker-compose.yml up -d   # from the repo root
//! while ! curl -sf http://localhost:9944/health > /dev/null 2>&1; do sleep 2; done
//! while ! curl -s --max-time 2 http://localhost:8088 > /dev/null 2>&1; do sleep 2; done
//! cargo run -p example-merge-transactions
//! docker compose -f devnet/docker-compose.yml down
//! ```

use midnight_provider::{MidnightProvider, Network, Seed};

mod counter {
    // Shared contract artifacts (see devnet/contracts/counter).
    compact_bindgen::contract!("../../devnet/contracts/counter/compiled/contract-info.json");
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

/// Dev node genesis wallet seed (funded with NIGHT + shielded test tokens).
const DEV_WALLET_SEED: &str = "0000000000000000000000000000000000000000000000000000000000000001";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Midnight Merge Transactions Example ===\n");

    let seed = Seed::from_hex(DEV_WALLET_SEED)?;
    let network = Network::Undeployed;
    let node_url = env_or("MIDNIGHT_NODE_URL", "ws://127.0.0.1:9944");
    let indexer_url = env_or("MIDNIGHT_INDEXER_URL", "http://127.0.0.1:8088");

    println!("0. Syncing wallet state from indexer...");
    let provider = MidnightProvider::new(&node_url, &indexer_url)?
        .sync_wallet(seed.clone(), &network)
        .await?;
    println!("   synced.\n");

    // Deploy via `send()` + `into_contract()` (rather than awaiting the builder)
    // so the provider is borrowed, not moved — we reuse it below to build the
    // transfer, merge, and submit.
    println!("1. Deploying counter contract...");
    let pending = counter::Contract::deploy(&provider)
        .with_initial_state(counter::LedgerInitialState::default())
        .with_zk_config(ZK_KEYS_DIR)
        .send()
        .await?;
    let (_best, pending) = pending.wait_best().await?;
    let contract = pending.into_contract().await?;
    println!("   address: {}\n", contract.address());

    // Build (and prove) the increment call without submitting. `build_increment`
    // is generated alongside the submitting `increment`; it returns the proven
    // transaction bytes for you to merge/submit yourself.
    println!("2. Building (not submitting) the increment call...");
    let call_tx: Vec<u8> = contract.circuits().build_increment().await?;
    println!("   proven call tx: {} bytes\n", call_tx.len());

    // Build (and prove) a second transaction without submitting. Here it is a
    // shielded self-transfer of 1 unit; in a real flow this is the other party's
    // already-proven transaction, received as bytes.
    println!("3. Building (not submitting) a shielded self-transfer...");
    let coin = provider
        .balance()
        .await?
        .shielded
        .coins
        .first()
        .cloned()
        .ok_or("wallet has no shielded coins — is this a fresh local devnet?")?;
    let recipient = seed.shielded_address(&network);
    let transfer = provider
        .transfer_shielded(coin.token_type, 1, &recipient)
        .build()
        .await?;
    println!("   proven transfer tx: {} bytes\n", transfer.tx_bytes.len());

    // Merge the two proven transactions into one. Merging combines their intents
    // and Zswap offers and sums their binding randomness; each transaction funds
    // its own side (including fees), so the merged transaction stays balanced.
    println!("4. Merging the two transactions...");
    let merged = provider.merge_transactions(&[call_tx, transfer.tx_bytes])?;
    println!("   merged tx: {} bytes\n", merged.len());

    // Submit the merged transaction: the increment and the transfer apply
    // atomically — either both land or neither does.
    println!("5. Submitting the merged transaction...");
    let pending = provider.submit(&merged).await?;
    println!("   ext hash:  {}", pending.extrinsic_hash_hex());
    let (best, pending) = pending.wait_best().await?;
    println!("   best:      {}", hex::encode(best.block_hash));
    let (finalized, _) = pending.wait_finalized().await?;
    println!("   finalized: {}", hex::encode(finalized.block_hash));

    println!("\n   counter round = {}", contract.ledger().await?.round()?);
    println!("\n=== Done ===");
    Ok(())
}
