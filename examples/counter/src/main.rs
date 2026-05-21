//! Counter contract example — deploy to a dev node and interact.
//!
//! ```bash
//! cd examples/counter && docker compose up -d
//! while ! curl -sf http://localhost:9944/health > /dev/null 2>&1; do sleep 2; done
//! cargo run -p example-counter
//! docker compose down
//! ```

use midnight_bindgen::hex;
use midnight_node_ledger_helpers::WalletSeed;
use midnight_provider::MidnightProvider;
use midnight_wallet::Wallet;

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

    // Sync the wallet against the indexer; this populates zswap, dust, and
    // unshielded state in one async constructor.
    println!("0. Syncing wallet state from indexer...");
    let seed = WalletSeed::try_from_hex_str(DEV_WALLET_SEED)?;
    let wallet = Wallet::sync(NODE_URL, INDEXER_URL, seed, "undeployed", None).await?;

    let provider = MidnightProvider::new(NODE_URL, INDEXER_URL)?.with_wallet(wallet);
    let witnesses = midnight_contract::interpreter::NoWitnesses;

    // 1. Deploy the contract; observe Best then Finalized inclusion.
    println!("1. Deploying counter contract...");
    let pending = counter::Contract::deploy(&provider)
        .with_initial_state(counter::LedgerInitialState::default())
        .with_zk_keys(ZK_KEYS_DIR)
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
    let returned: u64 = contract.circuits(&witnesses).increment().await?;
    println!("   returned = {returned}");
    println!("   round = {}", contract.ledger().await?.round()?);

    // 3. Call increment_by with an argument (returns the amount)
    println!("3. Calling increment_by(5) on-chain...");
    let returned: u16 = contract.circuits(&witnesses).increment_by(5).await?;
    println!("   returned = {returned}");
    println!("   round = {}", contract.ledger().await?.round()?);

    // To reference an existing contract (e.g. from a different process):
    // let contract = counter::Contract::at(&provider, &address)
    //     .with_zk_keys(ZK_KEYS_DIR)
    //     .build();

    println!("\n=== Done ===");
    Ok(())
}
