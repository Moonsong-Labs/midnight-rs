//! Counter contract example — deploy to a dev node and interact.
//!
//! ```bash
//! cd examples/counter && docker compose up -d
//! while ! curl -sf http://localhost:9944/health > /dev/null 2>&1; do sleep 2; done
//! cargo run -p example-counter
//! docker compose down
//! ```

use midnight_provider::MidnightProvider;

mod counter {
    midnight_bindgen::contract!("compiled/contract-info.json");
}

const NODE_URL: &str = "ws://127.0.0.1:9944";
const INDEXER_URL: &str = "http://127.0.0.1:8088";
const ZK_KEYS_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/compiled");

/// Dev node genesis wallet seed (funded with NIGHT tokens at genesis).
const DEV_WALLET_SEED: &str = "0000000000000000000000000000000000000000000000000000000000000001";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Midnight Counter Example ===\n");

    let provider = MidnightProvider::new(NODE_URL, INDEXER_URL)?.with_wallet(DEV_WALLET_SEED);
    let witnesses = midnight_contract::interpreter::NoWitnesses;

    // 1. Deploy the contract
    println!("1. Deploying counter contract...");
    let contract = counter::Contract::deploy(&provider)
        .with_initial_state(counter::LedgerInitialState::default())
        .with_zk_keys(ZK_KEYS_DIR)
        .await?;
    let address = contract.address().to_string();
    println!("   Deployed at: {address}");
    println!("   round = {}", contract.ledger().await?.round()?);

    // 2. Call increment on-chain (returns the increment amount)
    println!("2. Calling increment on-chain...");
    let returned: u64 = contract.circuits(&witnesses).increment().await?;
    println!("   returned = {returned}");
    println!("   round = {}", contract.ledger().await?.round()?);

    // 3. Call increment_by with an argument (returns the amount)
    println!("3. Calling increment_by(5) on-chain...");
    let returned: u16 = contract.circuits(&witnesses).increment_by(5).await?;
    println!("   returned = {returned}");
    println!("   round = {}", contract.ledger().await?.round()?);

    // 4. Lower-level: re-deploy via submit() and observe both Best and Finalized.
    println!("4. Re-deploying via lower-level submit to observe Best vs Finalized...");
    use midnight_bindgen::{ContractState, InMemoryDB, hex};
    use midnight_contract::{Prover, call};
    use std::path::Path;

    let raw_state: ContractState<InMemoryDB> = counter::LedgerInitialState::default().into();
    let state = call::with_zk_keys(raw_state, Path::new(ZK_KEYS_DIR))?;
    let result = call::deploy_funded(
        &state,
        NODE_URL,
        DEV_WALLET_SEED,
        Path::new(ZK_KEYS_DIR),
        &Prover::Local,
    )
    .await?;
    let mut pending = result.submit(NODE_URL).await?;
    println!("   ext hash: {}", pending.extrinsic_hash_hex());
    let best = pending.wait_best().await?;
    println!(
        "   in best block:      0x{}",
        hex::encode(best.block_hash.as_ref())
    );
    let finalized = pending.wait_finalized().await?;
    println!(
        "   in finalized block: 0x{}",
        hex::encode(finalized.block_hash.as_ref())
    );

    // To reference an existing contract (e.g. from a different process):
    // let contract = counter::Contract::at(&provider, &address)
    //     .with_zk_keys(ZK_KEYS_DIR)
    //     .build();

    println!("\n=== Done ===");
    Ok(())
}
