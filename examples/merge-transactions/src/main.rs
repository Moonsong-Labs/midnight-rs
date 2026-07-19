//! Merge transactions example — two wallets, one atomic transaction.
//!
//! Two independent wallets each build and prove their own transaction, then the
//! transactions are merged into one and submitted so they land atomically (all
//! or nothing). This is the multi-party building block behind swaps: each party
//! signs its own side with its own keys, and [`MidnightProvider::merge_transactions`]
//! combines them.
//!
//! Roles:
//! - Wallet A (seed 1, funded at genesis): deploys the counter contract and
//!   builds an `increment().build()` call (build-only, i.e. proved but not
//!   submitted). It also bootstraps wallet B, since the local `dev` preset only
//!   funds seed 1.
//! - Wallet B (seed 2): builds a shielded self-transfer (build-only) as its
//!   contribution.
//!
//! A then merges A's call with B's transfer and submits the result.
//!
//! The two new pieces this demonstrates:
//! - `contract.circuits().<circuit>().build()` — build + prove a contract call and
//!   return its bytes *without* submitting.
//! - `provider.merge_transactions(&[..])` — combine proven transactions into one.
//!
//! Fees: each wallet pays its own (the symmetric merge sums two self-funded
//! transactions). Having only seed 1 pay all the fees while spending seed 2's
//! coin is a different operation (balancing an external party's unbalanced
//! transaction) that the SDK does not support yet — see issue #127.
//!
//! Note: the wallet-B bootstrap (funding seed 2 with NIGHT + a shielded coin and
//! registering dust) is devnet- and timing-dependent; amounts and the post-fund
//! resync may need tuning for your devnet. A wallet cannot build a transaction
//! until it has dust to pay fees.
//!
//! ```bash
//! docker compose -f devnet/docker-compose.yml up -d   # from the repo root
//! while ! curl -sf http://localhost:9944/health > /dev/null 2>&1; do sleep 2; done
//! while ! curl -s --max-time 2 http://localhost:8088 > /dev/null 2>&1; do sleep 2; done
//! cargo run -p example-merge-transactions
//! docker compose -f devnet/docker-compose.yml down
//! ```

use midnight_provider::{MidnightProvider, NIGHT, Network, Seed};

mod counter {
    // Shared contract artifacts (see devnet/contracts/counter).
    compact_bindgen::contract!("../../devnet/contracts/counter/compiled/contract-info.json");
}

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_string())
}

const ZK_KEYS_DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../devnet/contracts/counter/compiled"
);

/// Genesis-funded dev wallet (NIGHT + shielded test tokens).
const SEED_A: &str = "0000000000000000000000000000000000000000000000000000000000000001";
/// Second wallet, unfunded on the `dev` preset — bootstrapped from A below.
const SEED_B: &str = "0000000000000000000000000000000000000000000000000000000000000002";

/// How much NIGHT A sends B so B can register + generate dust for its own fees.
const NIGHT_TO_B: u128 = 1_000_000;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Midnight Merge Transactions Example (two wallets) ===\n");

    let network = Network::Undeployed;
    let node_url = env_or("MIDNIGHT_NODE_URL", "ws://127.0.0.1:9944");
    let indexer_url = env_or("MIDNIGHT_INDEXER_URL", "http://127.0.0.1:8088");

    let seed_a = Seed::from_hex(SEED_A)?;
    let seed_b = Seed::from_hex(SEED_B)?;

    println!("0. Syncing both wallets...");
    let provider_a = MidnightProvider::new(&node_url, &indexer_url)?
        .sync_wallet(seed_a.clone(), &network)
        .await?;
    let provider_b = MidnightProvider::new(&node_url, &indexer_url)?
        .sync_wallet(seed_b.clone(), &network)
        .await?;
    println!("   A and B synced.\n");

    // --- Bootstrap wallet B from A (dev preset funds only seed 1) ---
    // B needs a shielded coin to spend and dust to pay its own fees.
    let coin = provider_a
        .balance()
        .await?
        .shielded
        .coins
        .first()
        .cloned()
        .ok_or("wallet A has no shielded coins — is this a fresh local devnet?")?;

    println!("1. Funding wallet B from A...");
    provider_a
        .transfer_unshielded(NIGHT, NIGHT_TO_B, &seed_b.unshielded_address(&network))
        .await?
        .wait_finalized()
        .await?;
    provider_a
        .transfer_shielded(coin.token_type, 2, &seed_b.shielded_address(&network))
        .await?
        .wait_finalized()
        .await?;
    provider_b.resync_wallet().await?;
    println!("   B funded; registering dust...");
    provider_b
        .register_dust(None)
        .await?
        .wait_finalized()
        .await?;
    provider_b.resync_wallet().await?;
    println!("   B ready.\n");

    // --- Wallet A: deploy the contract and build (not submit) the call ---
    println!("2. A deploys the counter and builds the increment call (build-only)...");
    let pending = counter::Contract::deploy(&provider_a)
        .with_initial_state(counter::LedgerInitialState::default())
        .with_zk_config(ZK_KEYS_DIR)
        .send()
        .await?;
    let (_best, pending) = pending.wait_best().await?;
    let contract = pending.into_contract().await?;
    let call_tx: Vec<u8> = contract.circuits().increment().build().await?;
    println!(
        "   contract {} — proven call tx: {} bytes\n",
        contract.address(),
        call_tx.len()
    );

    // --- Wallet B: build (not submit) its own contribution ---
    println!("3. B builds a shielded self-transfer (build-only)...");
    let transfer_tx = provider_b
        .transfer_shielded(coin.token_type, 1, &seed_b.shielded_address(&network))
        .build()
        .await?
        .tx_bytes;
    println!("   proven transfer tx: {} bytes\n", transfer_tx.len());

    // --- Merge the two parties' transactions and submit as one ---
    println!("4. A merges both transactions and submits...");
    let merged = provider_a.merge_transactions(&[call_tx, transfer_tx])?;
    let pending = provider_a.submit(&merged).await?;
    println!("   ext hash:  {}", pending.extrinsic_hash_hex());
    let (best, pending) = pending.wait_best().await?;
    println!("   best:      {}", hex::encode(best.block_hash));
    let (finalized, _) = pending.wait_finalized().await?;
    println!("   finalized: {}", hex::encode(finalized.block_hash));

    println!("\n   counter round = {}", contract.ledger().await?.round()?);
    println!("\n=== Done ===");
    Ok(())
}
