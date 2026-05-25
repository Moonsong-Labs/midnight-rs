//! Contract maintenance example — deploy a governable contract, then rotate a
//! verifier key and replace the maintenance authority.
//!
//! A contract's maintenance authority is a k-of-n committee allowed to change
//! its verifier keys or hand control to a new committee. This SDK holds no
//! signing key: you set the committee (public keys) at deploy and sign each
//! maintenance update externally.
//!
//! ```bash
//! cd examples/contract-maintenance && docker compose up -d
//! while ! curl -sf http://localhost:9944/health > /dev/null 2>&1; do sleep 2; done
//! while ! curl -s --max-time 2 http://localhost:8088 > /dev/null 2>&1; do sleep 2; done
//! cargo run -p example-contract-maintenance
//! docker compose down
//! ```

use midnight_bindgen::{ContractMaintenanceAuthority, ContractState, StateValue, StorageHashMap};
use midnight_contract::{Contract, SigningKey};
use midnight_provider::{MidnightProvider, Network, WalletSeed};

const NODE_URL: &str = "ws://127.0.0.1:9944";
const INDEXER_URL: &str = "http://127.0.0.1:8088";
const ZK_KEYS_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/compiled");

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

    // 1. Deploy a contract governed by a 1-of-1 committee (just `authority`).
    //    `with_zk_keys` loads the compiled verifier keys, so the deployed
    //    contract has the `increment` and `increment_by` circuits defined.
    println!("1. Deploying a governable contract...");
    let initial = ContractState::new(
        StateValue::Array(vec![StateValue::from(0u64)].into()),
        StorageHashMap::new(),
        ContractMaintenanceAuthority::default(),
    );
    let pending = Contract::deploy(&provider)
        .with_initial_state(initial)
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
    contract: &Contract<&MidnightProvider>,
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
