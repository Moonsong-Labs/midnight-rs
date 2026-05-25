//! Contract maintenance example — deploy a governable contract, then rotate a
//! verifier key and replace the maintenance authority.
//!
//! A contract's maintenance authority is a k-of-n committee allowed to change
//! its verifier keys or hand control to a new committee. This SDK holds no
//! signing key: you set the committee (public keys) at deploy and sign each
//! maintenance update externally.
//!
//! Reuses the counter contract, so deploying it gives a contract with the
//! `increment` / `increment_by` circuits to rotate. Runs against the shared
//! local devnet (the repo-root `docker-compose.yml`); see README.md.

use midnight_contract::SigningKey;
use midnight_provider::{MidnightProvider, Network, WalletSeed};

mod counter {
    // Shared contract artifacts (see examples/contracts/counter), reused by the
    // counter example too.
    midnight_bindgen::contract!("../contracts/counter/compiled/contract-info.json");
}

const NODE_URL: &str = "ws://127.0.0.1:9944";
const INDEXER_URL: &str = "http://127.0.0.1:8088";
const ZK_KEYS_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../contracts/counter/compiled");

/// Dev node genesis wallet seed (funded with NIGHT tokens at genesis).
const DEV_WALLET_SEED: &str = "0000000000000000000000000000000000000000000000000000000000000001";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Midnight Contract Maintenance Example ===\n");

    println!("0. Syncing wallet state from indexer...");
    let seed = WalletSeed::try_from_hex_str(DEV_WALLET_SEED)?;
    let provider = MidnightProvider::new(NODE_URL, INDEXER_URL)?
        .sync_wallet(seed, Network::Undeployed)
        .await?;
    println!("   synced.\n");

    // The deployer owns the authority signing key; the chain only learns its
    // public half. For a multi-party committee you would collect the members'
    // verifying keys here and pick a threshold > 1.
    let authority = SigningKey::sample(rand::thread_rng());

    // 1. Deploy the counter contract governed by a 1-of-1 committee.
    println!("1. Deploying a governable contract...");
    let pending = counter::Contract::deploy(&provider)
        .with_initial_state(counter::LedgerInitialState::default())
        .with_zk_keys(ZK_KEYS_DIR)
        .with_maintenance_authority(vec![authority.verifying_key()], 1)
        .send()
        .await?;
    let (_, pending) = pending.wait_best().await?;
    let contract = pending.into_contract().await?;
    println!("   address: {}", contract.address());
    print_authority(&contract).await?;

    // 2. Rotate the `increment` verifier key: remove + insert in one signed,
    //    atomic update (insert never replaces, so it must follow a remove).
    println!("\n2. Rotating the `increment` verifier key (remove + insert)...");
    let vk_bytes = std::fs::read(format!("{ZK_KEYS_DIR}/keys/increment.verifier"))?;
    contract
        .maintenance()
        .remove_verifier_key("increment")
        .insert_verifier_key("increment", vk_bytes)
        .prepare()
        .await?
        .sign(0, &authority) // current authority signs at committee index 0
        .await?
        .wait_best()
        .await?;
    println!("   rotated.");
    print_authority(&contract).await?;

    // 3. Hand control to a fresh committee.
    println!("\n3. Replacing the maintenance authority...");
    let new_authority = SigningKey::sample(rand::thread_rng());
    contract
        .maintenance()
        .replace_authority(vec![new_authority.verifying_key()], 1)
        .prepare()
        .await?
        .sign(0, &authority) // the *current* authority authorizes the handover
        .await?
        .wait_best()
        .await?;
    println!("   replaced. Future updates must be signed by the new authority.");
    print_authority(&contract).await?;

    println!("\n=== Done ===");
    Ok(())
}

/// Print the contract's current committee size, threshold, and counter.
async fn print_authority(
    contract: &counter::Contract<&MidnightProvider>,
) -> Result<(), Box<dyn std::error::Error>> {
    let a = contract.maintenance_authority().await?;
    println!(
        "   authority: {} member(s), threshold {}, counter {}",
        a.committee.len(),
        a.threshold,
        a.counter
    );
    Ok(())
}
