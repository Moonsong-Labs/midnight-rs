//! Combine-and-sponsor example — one party acts, another pays.
//!
//! B wants to do two things atomically, a shielded transfer and a contract
//! call, but holds **no Dust**, so it can't pay fees. A (funded at genesis)
//! sponsors: it combines B's two proven, fee-less transactions into one and
//! pays all the fees. B never needs Dust of its own.
//!
//! Roles:
//! - A (seed 1, genesis-funded with NIGHT + Dust): deploys the counter, and is
//!   the fee payer. It seeds B with a shielded coin to spend.
//! - B (seed 2, no Dust): fills a fee-less contract call and a fee-less
//!   shielded transfer, and hands both to A.
//!
//! The pieces this demonstrates:
//! - A fee-less contract call (`.without_fee()` on the call builder) and a
//!   fee-less transfer (`TransactionRequest::without_fee`). Each gives a
//!   `TxEnvelope` whose bytes (`into_bytes`) go to the payer.
//! - `merge_transactions(&[..])` (↔ midnight-js `Transaction.merge`):
//!   combine proven transactions into one that lands atomically.
//! - `TransactionRequest::sponsor(bytes)` (↔ midnight-js
//!   `walletProvider.balanceTx`): pay another party's Dust fees.
//!   `send_transaction` sends the result, and hands the fee Dust back when
//!   the node never gets the transaction or rejects it.
//!
//! Nobody but the genesis-funded A needs Dust, so this runs on a fresh devnet
//! (a freshly funded wallet accrues Dust only over ~a week, so B could not
//! self-fund here anyway — which is exactly what sponsoring is for).
//!
//! ```bash
//! make dev-up   # from the repo root: starts the devnet and waits for a block
//! make run-combine-and-sponsor   # from the repo root
//! make dev-down   # from the repo root
//! ```

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, bail};
use midnight_core::provider::{TransactionRequest, merge_transactions};
use midnight_core::{MidnightProvider, Seed, Wallet};

mod counter {
    // Shared contract artifacts (see devnet/contracts/counter).
    midnight_core::contract!("../../devnet/contracts/counter/compiled/compiler/analyzed-ir.sexp");
}

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_string())
}

/// Genesis-funded dev wallet (NIGHT + Dust + shielded test tokens).
const SEED_A: &str = "0000000000000000000000000000000000000000000000000000000000000001";
/// Second wallet. It holds no Dust, so it fills only fee-less transactions.
const SEED_B: &str = "0000000000000000000000000000000000000000000000000000000000000002";

/// Shielded units A seeds B with, so B has a coin to transfer.
const SHIELDED_TO_B: u128 = 3;

/// The pause between two passes of B's background sync. A short pause ends
/// the wait soon after the indexer serves the transaction.
const SYNC_PAUSE: Duration = Duration::from_secs(1);

/// How long a wallet waits to see the effect of a finalized transaction.
const EFFECT_TIMEOUT: Duration = Duration::from_secs(60);

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("=== Midnight Combine-and-Sponsor Example (B acts, A pays) ===\n");

    let node_url = env_or("MIDNIGHT_NODE_URL", "ws://127.0.0.1:9944");
    let indexer_url = env_or("MIDNIGHT_INDEXER_URL", "http://127.0.0.1:8088");

    println!("0. Syncing both wallets...");
    let provider_a = MidnightProvider::new(&node_url);
    let wallet_a = Wallet::from_seed(Seed::from_hex(SEED_A)?)
        .sync(&provider_a, &indexer_url)
        .await?;
    let recipient_a = wallet_a.shielded_recipient();
    let provider_a = provider_a.with_wallet(&wallet_a);
    let provider_b = MidnightProvider::new(&node_url);
    let wallet = Wallet::from_seed(Seed::from_hex(SEED_B)?)
        .sync(&provider_b, &indexer_url)
        .await?;
    let recipient_b = wallet.shielded_recipient();
    let wallet_b = Arc::new(wallet);
    let provider_b = Arc::new(provider_b.with_wallet(Arc::clone(&wallet_b)));
    println!("   A and B synced.\n");

    // --- A deploys the counter (A is genesis-funded with Dust) ---
    println!("1. A deploys the counter...");
    let contract = counter::Contract::deploy(&provider_a)
        .with_zk_config(counter::ZK_CONFIG_DIR)
        .send()
        .await?
        .into_contract()
        .await?;
    let address = contract.address().to_string();
    // Baseline the counter before B's increment lands, so the final assertion
    // proves the sponsored call actually applied (round moved by exactly one),
    // not just that some transaction finalized.
    let initial_round = contract.ledger().await?.round()?;
    println!("   contract {address} (round {initial_round})\n");

    // --- A seeds B with a shielded coin (B needs no Dust, only something to
    //     spend) ---
    let coin = wallet_a
        .balance()
        .await
        .shielded
        .coins
        .first()
        .cloned()
        .context("wallet A has no shielded coins. Is this a fresh local devnet?")?;
    println!("2. A sends B a shielded coin...");
    // B can hold this token from an earlier run, so B waits for its total to
    // grow, not for any coin of the token.
    let b_before = wallet_b.balance().await.shielded_value(coin.token_type);
    provider_a
        .send_transaction(TransactionRequest::shielded(
            coin.token_type,
            SHIELDED_TO_B,
            recipient_b,
        ))
        .await?
        .get_receipt()
        .await?;
    let sync_b = wallet_b.sync_in_background(provider_b.clone(), SYNC_PAUSE);
    tokio::time::timeout(
        EFFECT_TIMEOUT,
        sync_b.wait_until(|b| b.shielded_value(coin.token_type) >= b_before + SHIELDED_TO_B),
    )
    .await
    .map_err(|elapsed| {
        sync_b
            .last_error()
            .map_or_else(|| elapsed.into(), anyhow::Error::from)
    })
    .context("B did not see the coin from A")??;
    println!("   B holds a coin (and no Dust).\n");

    // --- B fills its two fee-less transactions (it pays nothing) ---
    println!("3. B fills a fee-less contract call (increment)...");
    let contract_b = counter::Contract::at(&provider_b, address.as_str())
        .with_zk_config(counter::ZK_CONFIG_DIR)
        .build();
    // B's inputs stay reserved until a sync sees them spent or their TTL ends.
    let call_tx = contract_b
        .circuits()
        .increment()
        .without_fee()
        .await?
        .into_bytes();
    println!("   proven, fee-less call: {} bytes", call_tx.len());

    println!("   B fills a fee-less shielded transfer to A...");
    let transfer_tx = provider_b
        .fill(TransactionRequest::shielded(coin.token_type, 1, recipient_a).without_fee())
        .await?
        .into_bytes();
    println!(
        "   proven, fee-less transfer: {} bytes\n",
        transfer_tx.len()
    );

    // --- A combines B's transactions and pays all the fees ---
    println!("4. A merges B's two transactions and sponsors the fees...");
    let merged = merge_transactions(&[call_tx, transfer_tx])?;
    let pending = provider_a
        .send_transaction(TransactionRequest::sponsor(merged))
        .await?;
    println!("   ext hash:  {:x}", pending.extrinsic_hash());
    let receipt = pending.get_receipt().await?;
    println!("   finalized in {}\n", hex::encode(receipt.block_hash));

    // The whole point of the flow: B's fee-less increment, carried by A's
    // sponsored transaction, must have applied exactly once.
    let round = contract.ledger().await?.round()?;
    if round != initial_round + 1 {
        bail!(
            "counter did not advance by one: expected {}, got {round}",
            initial_round + 1
        );
    }
    println!("   counter round = {round} (was {initial_round})");
    println!("\n=== Done ===");
    Ok(())
}
