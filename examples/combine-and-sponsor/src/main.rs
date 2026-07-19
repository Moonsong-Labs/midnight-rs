//! Combine-and-sponsor example — one party acts, another pays.
//!
//! B wants to do two things atomically, a shielded transfer and a contract
//! call, but holds **no Dust**, so it can't pay fees. A (funded at genesis)
//! sponsors: it combines B's two proven-but-Dustless transactions into one and
//! pays all the fees. B never needs Dust of its own.
//!
//! Roles:
//! - A (seed 1, genesis-funded with NIGHT + Dust): deploys the counter, and is
//!   the fee payer. It seeds B with a shielded coin to spend.
//! - B (seed 2, no Dust): builds a Dustless contract call and a Dustless
//!   shielded transfer, and hands both to A.
//!
//! The pieces this demonstrates:
//! - `.without_dust()` on **both** a contract call and a transfer (the
//!   `DustlessBuilder` trait) — Dust is the general fee token, so it is not
//!   transaction-specific. Each yields a `DustlessTransaction`.
//! - `provider.merge_transactions(&[..])` (↔ midnight-js `Transaction.merge`) —
//!   combine proven transactions into one that lands atomically.
//! - `provider.balance_transaction(bytes)` (↔ midnight-js
//!   `walletProvider.balanceTransaction`) — pay another party's Dust fees.
//!
//! Nobody but the genesis-funded A needs Dust, so this runs on a fresh devnet
//! (a freshly funded wallet accrues Dust only over ~a week, so B could not
//! self-fund here anyway — which is exactly what sponsoring is for).
//!
//! ```bash
//! docker compose -f devnet/docker-compose.yml up -d   # from the repo root
//! while ! curl -sf http://localhost:9944/health > /dev/null 2>&1; do sleep 2; done
//! while ! curl -s --max-time 2 http://localhost:8088 > /dev/null 2>&1; do sleep 2; done
//! cargo run -p example-combine-and-sponsor
//! docker compose -f devnet/docker-compose.yml down
//! ```

use midnight_provider::{DustlessBuilder, MidnightProvider, Network, Seed, Verdict};

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

/// Genesis-funded dev wallet (NIGHT + Dust + shielded test tokens).
const SEED_A: &str = "0000000000000000000000000000000000000000000000000000000000000001";
/// Second wallet, holds no Dust — it only ever builds Dustless transactions.
const SEED_B: &str = "0000000000000000000000000000000000000000000000000000000000000002";

/// Shielded units A seeds B with, so B has a coin to transfer.
const SHIELDED_TO_B: u128 = 3;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Midnight Combine-and-Sponsor Example (B acts, A pays) ===\n");

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

    // --- A deploys the counter (A is genesis-funded with Dust) ---
    println!("1. A deploys the counter...");
    let pending = counter::Contract::deploy(&provider_a)
        .with_initial_state(counter::LedgerInitialState::default())
        .with_zk_config(ZK_KEYS_DIR)
        .send()
        .await?;
    let (_best, pending) = pending.wait_best().await?;
    let contract = pending.into_contract().await?;
    let address = contract.address().to_string();
    // Baseline the counter before B's increment lands, so the final assertion
    // proves the sponsored call actually applied (round moved by exactly one),
    // not just that some transaction finalized.
    let initial_round = contract.ledger().await?.round()?;
    println!("   contract {address} (round {initial_round})\n");

    // --- A seeds B with a shielded coin (B needs no Dust, only something to
    //     spend) ---
    let coin = provider_a
        .balance()
        .await?
        .shielded
        .coins
        .first()
        .cloned()
        .ok_or("wallet A has no shielded coins — is this a fresh local devnet?")?;
    println!("2. A sends B a shielded coin...");
    provider_a
        .transfer_shielded(
            coin.token_type,
            SHIELDED_TO_B,
            &seed_b.shielded_address(&network),
        )
        .await?
        .wait_finalized()
        .await?;
    provider_b.resync_wallet().await?;
    println!("   B holds a coin (and no Dust).\n");

    // --- B builds its two Dustless transactions (it pays nothing) ---
    println!("3. B builds a Dustless contract call (increment)...");
    let contract_b = counter::Contract::at(&provider_b, address.as_str())
        .with_zk_config(ZK_KEYS_DIR)
        .build();
    let call_tx = contract_b.circuits().increment().without_dust().await?;
    println!(
        "   proven, Dustless call: {} bytes",
        call_tx.as_bytes().len()
    );

    println!("   B builds a Dustless shielded transfer to A...");
    let transfer_tx = provider_b
        .transfer_shielded(coin.token_type, 1, &seed_a.shielded_address(&network))
        .without_dust()
        .await?;
    println!(
        "   proven, Dustless transfer: {} bytes\n",
        transfer_tx.as_bytes().len()
    );

    // --- A combines B's transactions and pays all the fees ---
    println!("4. A merges B's two transactions and sponsors the fees...");
    let merged =
        provider_a.merge_transactions(&[call_tx.into_bytes(), transfer_tx.into_bytes()])?;
    let sponsored = provider_a.balance_transaction(&merged).await?;
    let pending = provider_a.submit(&sponsored).await?;
    println!("   ext hash:  {}", pending.extrinsic_hash_hex());
    let (_best, pending) = pending.wait_best().await?;
    let (finalized, _) = pending.wait_finalized().await?;
    println!("   finalized in {}\n", hex::encode(finalized.block_hash));

    // `wait_finalized` returns Ok for any included transaction regardless of
    // outcome, so assert the verdict explicitly: a sponsored tx whose call
    // intent was dropped during merge/balance would otherwise finalize as a
    // green no-op and this example would pass while proving nothing.
    if finalized.verdict != Verdict::Success {
        return Err(format!(
            "sponsored transaction did not succeed: {:?}",
            finalized.verdict
        )
        .into());
    }

    // The whole point of the flow: B's Dustless increment, carried by A's
    // sponsored transaction, must have applied exactly once.
    let round = contract.ledger().await?.round()?;
    if round != initial_round + 1 {
        return Err(format!(
            "counter did not advance by one: expected {}, got {round}",
            initial_round + 1
        )
        .into());
    }
    println!("   counter round = {round} (was {initial_round})");
    println!("\n=== Done ===");
    Ok(())
}
