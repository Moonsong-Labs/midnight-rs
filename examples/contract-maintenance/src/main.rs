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
//! index is its position in the committee `Vec` passed at deploy. The members
//! here hold ECDSA keys, the kind a KMS or an HSM can hold, modeled with `k256`.
//! The SDK sees only their bytes: each public key in SEC1 form, and each
//! signature over SHA-256 of the payload as `r || s`. A Schnorr (BIP-340) member
//! works the same way.
//!
//! Reuses the counter contract, so deploying it gives a contract with the
//! `increment` / `increment_by` circuits to rotate. Runs against the shared
//! local devnet (`devnet/docker-compose.yml`); see README.md.

use k256::ecdsa::SigningKey;
use k256::ecdsa::signature::Signer;
use midnight_core::contract::{ContractMaintenanceVerifyingKey, parse_maintenance_key};
use midnight_core::{MidnightProvider, Seed, Wallet};

mod counter {
    // Shared contract artifacts (see devnet/contracts/counter), reused by the
    // counter example too.
    midnight_core::contract!("../../devnet/contracts/counter/compiled/compiler/analyzed-ir.sexp");
}

/// Node/indexer URLs default to the local devnet; override with the
/// `MIDNIGHT_NODE_URL` / `MIDNIGHT_INDEXER_URL` env vars to run elsewhere.
fn env_or(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_string())
}

/// Dev node genesis wallet seed (funded with NIGHT tokens at genesis).
const DEV_WALLET_SEED: &str = "0000000000000000000000000000000000000000000000000000000000000001";

/// Stands in for a committee member on a separate machine: it has only this
/// member's signing key and the payload to sign — no provider, no wallet, no
/// other member's key. It returns just the signature, which the coordinator
/// collects out of band.
fn member_sign(payload: &[u8], key: &SigningKey) -> [u8; 64] {
    let signature: k256::ecdsa::Signature = key.sign(payload);
    signature.to_bytes().into()
}

/// The committee entries of `members`, in order, read from their SEC1 public
/// keys.
fn committee(members: &[SigningKey]) -> anyhow::Result<Vec<ContractMaintenanceVerifyingKey>> {
    members
        .iter()
        .map(|key| {
            Ok(parse_maintenance_key(
                key.verifying_key().to_encoded_point(true).as_bytes(),
            )?)
        })
        .collect()
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("=== Midnight Contract Maintenance Example (2-of-3) ===\n");

    println!("0. Syncing wallet state from indexer...");
    let node_url = env_or("MIDNIGHT_NODE_URL", "ws://127.0.0.1:9944");
    let indexer_url = env_or("MIDNIGHT_INDEXER_URL", "http://127.0.0.1:8088");
    let provider = MidnightProvider::new(&node_url);
    let wallet = Wallet::from_seed(Seed::from_hex(DEV_WALLET_SEED)?)
        .sync(&provider, &indexer_url)
        .await?;
    let provider = provider.with_wallet(&wallet);
    println!("   synced.\n");

    // Three committee members, each modeling a key held on a separate machine.
    // We keep all three here only to drive the example end to end; in production
    // no single process holds more than one. Committee order defines the signer
    // indices: members[0] → 0, members[1] → 1, members[2] → 2.
    let members = [
        SigningKey::random(&mut rand::thread_rng()),
        SigningKey::random(&mut rand::thread_rng()),
        SigningKey::random(&mut rand::thread_rng()),
    ];

    // 1. Deploy the counter contract governed by a 2-of-3 committee.
    println!("1. Deploying a governable contract (2-of-3 committee)...");
    let contract = counter::Contract::deploy(&provider)
        .with_zk_config(counter::ZK_CONFIG_DIR)
        .with_maintenance_authority(committee(&members)?, 2)
        .send()
        .await?
        .into_contract()
        .await?;
    println!("   address: {}", contract.address());
    print_authority(&contract).await?;

    // 2. Rotate the `increment` verifier key: remove + insert in one signed,
    //    atomic update (insert never replaces, so it must follow a remove).
    //    Members 0 and 2 sign; member 1 is offline — 2 of 3 still clears.
    println!("\n2. Rotating the `increment` verifier key (members 0 and 2 sign)...");
    let vk_bytes = std::fs::read(format!(
        "{}/keys/increment.verifier",
        counter::ZK_CONFIG_DIR
    ))?;
    let prepared = contract
        .maintenance()
        .remove_verifier_key("increment")
        .insert_verifier_key("increment", vk_bytes)
        .prepare()
        .await?;

    // Coordinator hands the payload to each available member; they sign on their
    // own machines and return signatures.
    let payload = prepared.data_to_sign();
    let sig0 = member_sign(&payload, &members[0]);
    let sig2 = member_sign(&payload, &members[2]);

    // Coordinator attaches the quorum at each member's committee index, then
    // sends.
    prepared
        .add_signature(0, sig0)
        .add_signature(2, sig2)
        .send()
        .await?
        .get_receipt()
        .await?;
    println!("   rotated.");
    print_authority(&contract).await?;

    // 3. Hand control to a fresh 2-of-3 committee. The *current* committee
    //    authorizes the handover; this time members 0 and 1 sign (any two work).
    println!("\n3. Replacing the maintenance authority (members 0 and 1 sign)...");
    let new_members = [
        SigningKey::random(&mut rand::thread_rng()),
        SigningKey::random(&mut rand::thread_rng()),
        SigningKey::random(&mut rand::thread_rng()),
    ];
    let prepared = contract
        .maintenance()
        .replace_authority(committee(&new_members)?, 2)
        .prepare()
        .await?;

    let payload = prepared.data_to_sign();
    let sig0 = member_sign(&payload, &members[0]);
    let sig1 = member_sign(&payload, &members[1]);
    prepared
        .add_signature(0, sig0)
        .add_signature(1, sig1)
        .send()
        .await?
        .get_receipt()
        .await?;
    println!("   replaced. Future updates must be signed by the new committee.");
    print_authority(&contract).await?;

    println!("\n=== Done ===");
    Ok(())
}

/// Print the contract's current committee size, threshold, and counter.
async fn print_authority(
    contract: &counter::Contract<&MidnightProvider<&Wallet>>,
) -> anyhow::Result<()> {
    let a = contract.maintenance_authority().await?;
    println!(
        "   authority: {} member(s), threshold {}, counter {}",
        a.committee.len(),
        a.threshold,
        a.counter
    );
    Ok(())
}
