//! Temporary check for issue #155: prove a zkir-v3 circuit and submit it.
//!
//! Needs a proof server built with `--features experimental`, plus counter
//! artifacts compiled with `compactc --feature-zkir-v3`.
//!
//! ```bash
//! MIDNIGHT_NODE_URL=ws://127.0.0.1:19944 \
//! MIDNIGHT_INDEXER_URL=http://127.0.0.1:18088 \
//! MIDNIGHT_PROOF_SERVER=http://127.0.0.1:6301 \
//! V3_ZK_DIR=/path/to/counter-v3-full \
//!   cargo run -p example-counter --bin zkir_v3_e2e
//! ```

use std::sync::Arc;

use midnight_provider::{MidnightProvider, Network, RemoteProofServer, Seed};

mod counter {
    // normalized-ir.sexp is byte-identical between a v2 and a v3 compile, so
    // the same bindings drive both. Only keys/ and zkir/ differ.
    compact_bindgen::contract!("../../devnet/contracts/counter/compiled/normalized-ir.sexp");
}

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_string())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let node_url = env_or("MIDNIGHT_NODE_URL", "ws://127.0.0.1:19944");
    let indexer_url = env_or("MIDNIGHT_INDEXER_URL", "http://127.0.0.1:18088");
    let proof_url = env_or("MIDNIGHT_PROOF_SERVER", "http://127.0.0.1:6301");
    let zk_dir = std::env::var("V3_ZK_DIR").expect("set V3_ZK_DIR to the v3 compile output");

    println!("node   {node_url}");
    println!("proof  {proof_url}");
    println!("zk     {zk_dir}\n");

    let seed = Seed::from_hex("0000000000000000000000000000000000000000000000000000000000000001")?;
    let provider = MidnightProvider::new(&node_url, &indexer_url)?
        .with_proof_provider(Arc::new(RemoteProofServer::new(proof_url)))
        .sync_wallet(seed, Network::Undeployed)
        .await?;

    println!("1. deploying the v3-compiled counter ...");
    let pending = counter::Contract::deploy(&provider)
        .with_initial_state(counter::LedgerInitialState::default())
        .with_zk_config(zk_dir.as_str())
        .send()
        .await?;
    let (_, pending) = pending.wait_best().await?;
    let (finalized, pending) = pending.wait_finalized().await?;
    println!("   finalized {}", hex::encode(finalized.block_hash));
    let contract = pending.into_contract().await?;
    println!("   address   {}", contract.address());
    println!("   round     {}\n", contract.ledger().await?.round()?);

    println!("2. calling increment ...");
    let call = contract.circuits().increment().await?;
    println!("   returned  {}", call.value);
    println!("   tx        {}", hex::encode(call.extrinsic_hash));
    println!("   round     {}\n", contract.ledger().await?.round()?);

    println!("zkir-v3 circuit proved and accepted on chain");
    Ok(())
}
