//! Contract maintenance example — deploy a governable contract, then rotate a
//! verifier key and replace the maintenance authority, governed by a 2-of-3
//! committee whose keys live on separate machines.
//!
//! A contract's maintenance authority is a k-of-n committee allowed to change
//! its verifier keys or hand control to a new committee. This SDK holds no
//! signing key, and a real committee never gathers the keys in one place. The
//! flow is therefore always three-sided:
//!
//!   coordinator: `prepare()`, then hand out `data_to_sign()` (the payload)
//!   each member: signs the payload on their own machine, returns a signature
//!   coordinator: `add_signature(index, sig)` for a quorum, then submits
//!
//! With 2-of-3 any two members suffice, so one can be offline. A member's signer
//! index is its position in the committee `Vec` passed at deploy.
//!
//! Reuses the counter contract, so deploying it gives a contract with the
//! `increment` / `increment_by` circuits to rotate. Runs against the shared
//! local devnet (the repo-root `docker-compose.yml`); see README.md.

use midnight_contract::{Signature, SigningKey};
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

/// Stands in for a committee member on a separate machine: it has only this
/// member's signing key and the payload to sign — no provider, no wallet, no
/// other member's key. It returns just the signature, which the coordinator
/// collects out of band.
fn member_sign(key: &SigningKey, payload: &[u8]) -> Signature {
    key.sign(&mut rand::thread_rng(), payload)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Midnight Contract Maintenance Example (2-of-3) ===\n");

    println!("0. Syncing wallet state from indexer...");
    let seed = WalletSeed::try_from_hex_str(DEV_WALLET_SEED)?;
    let provider = MidnightProvider::new(NODE_URL, INDEXER_URL)?
        .sync_wallet(seed, Network::Undeployed)
        .await?;
    println!("   synced.\n");

    // Three committee members, each modeling a key held on a separate machine.
    // We keep all three here only to drive the example end to end; in production
    // no single process holds more than one. Committee order defines the signer
    // indices: members[0] → 0, members[1] → 1, members[2] → 2.
    let members = [
        SigningKey::sample(rand::thread_rng()),
        SigningKey::sample(rand::thread_rng()),
        SigningKey::sample(rand::thread_rng()),
    ];
    let committee: Vec<_> = members.iter().map(|k| k.verifying_key()).collect();

    // 1. Deploy the counter contract governed by a 2-of-3 committee.
    println!("1. Deploying a governable contract (2-of-3 committee)...");
    let pending = counter::Contract::deploy(&provider)
        .with_initial_state(counter::LedgerInitialState::default())
        .with_zk_keys(ZK_KEYS_DIR)
        .with_maintenance_authority(committee, 2)
        .send()
        .await?;
    let (_, pending) = pending.wait_best().await?;
    let contract = pending.into_contract().await?;
    println!("   address: {}", contract.address());
    print_authority(&contract).await?;

    // 2. Rotate the `increment` verifier key: remove + insert in one signed,
    //    atomic update (insert never replaces, so it must follow a remove).
    //    Members 0 and 2 sign; member 1 is offline — 2 of 3 still clears.
    println!("\n2. Rotating the `increment` verifier key (members 0 and 2 sign)...");
    let vk_bytes = std::fs::read(format!("{ZK_KEYS_DIR}/keys/increment.verifier"))?;
    let prepared = contract
        .maintenance()
        .remove_verifier_key("increment")
        .insert_verifier_key("increment", vk_bytes)
        .prepare()
        .await?;

    // Coordinator hands the payload to each available member; they sign on their
    // own machines and return signatures.
    let payload = prepared.data_to_sign();
    let sig0 = member_sign(&members[0], &payload);
    let sig2 = member_sign(&members[2], &payload);

    // Coordinator attaches the quorum at each member's committee index, then
    // builds + submits.
    prepared
        .add_signature(0, sig0)
        .add_signature(2, sig2)
        .await?
        .wait_best()
        .await?;
    println!("   rotated.");
    print_authority(&contract).await?;

    // 3. Hand control to a fresh 2-of-3 committee. The *current* committee
    //    authorizes the handover; this time members 0 and 1 sign (any two work).
    println!("\n3. Replacing the maintenance authority (members 0 and 1 sign)...");
    let new_members = [
        SigningKey::sample(rand::thread_rng()),
        SigningKey::sample(rand::thread_rng()),
        SigningKey::sample(rand::thread_rng()),
    ];
    let new_committee: Vec<_> = new_members.iter().map(|k| k.verifying_key()).collect();
    let prepared = contract
        .maintenance()
        .replace_authority(new_committee, 2)
        .prepare()
        .await?;

    let payload = prepared.data_to_sign();
    let sig0 = member_sign(&members[0], &payload);
    let sig1 = member_sign(&members[1], &payload);
    prepared
        .add_signature(0, sig0)
        .add_signature(1, sig1)
        .await?
        .wait_best()
        .await?;
    println!("   replaced. Future updates must be signed by the new committee.");
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
