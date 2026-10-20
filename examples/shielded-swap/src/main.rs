//! Shielded swap example: a native two-party cross-token token swap.
//!
//! A and B each hold a different shielded token and want to trade, without
//! trusting each other and without either paying fees. Each builds one *half*
//! of the swap: a proven, fee-less Zswap transaction that gives one token and
//! takes the other, net unbalanced. The two halves are exact mirrors, so
//! merging them cancels both tokens into a balanced transaction a sponsor funds
//! and submits.
//!
//! The swap itself is the three calls in `main`:
//! - `shielded_swap(give, give_amount, receive, receive_amount)`: build one
//!   fee-less half as a `DustlessTransaction`.
//! - `merge_transactions(&[..])`: fold the two mirrored halves into one balanced
//!   transaction (the mirror deltas cancel, so no token deficit remains).
//! - `balance_transaction(bytes)`: a sponsor pays the merged swap's Dust fees.
//!
//! Everything before that is setup: giving B a second token to trade (see
//! `mint`), so it does not clutter the flow.
//!
//! ```bash
//! docker compose -f devnet/docker-compose.yml up -d   # from the repo root
//! while ! curl -sf http://localhost:9944/health > /dev/null 2>&1; do sleep 2; done
//! while ! curl -s --max-time 2 http://localhost:8088 > /dev/null 2>&1; do sleep 2; done
//! make run-shielded-swap   # from the repo root
//! docker compose -f devnet/docker-compose.yml down
//! ```

mod mint;

use anyhow::{Context, ensure};
use midnight_core::provider::WalletBalance;
use midnight_core::{MidnightProvider, Seed, Wallet, WalletBuilder};

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_string())
}

/// Genesis-funded dev wallet (NIGHT + Dust + a native shielded token).
const SEED_A: &str = "0000000000000000000000000000000000000000000000000000000000000001";
/// Second wallet, holds no Dust; it only ever builds a fee-less swap half.
const SEED_B: &str = "0000000000000000000000000000000000000000000000000000000000000002";

/// Units of token Y minted to B, enough to give `DY` and keep a remainder.
const MINT_Y: u64 = 1000;
/// A gives `DX` of X and receives `DY` of Y; B mirrors.
const DX: u128 = 2;
const DY: u128 = 5;

/// Resync `provider` until `seen` holds for its balance, and return that
/// balance.
///
/// A finalized transaction reaches the indexer, which a resync reads, a moment
/// after the node reports it, so a single resync can miss it. Polling for the
/// effect needs no promise about when the indexer catches up.
async fn resync_until(
    provider: &MidnightProvider,
    seen: impl Fn(&WalletBalance) -> bool,
) -> anyhow::Result<WalletBalance> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        provider.resync_wallet().await?;
        let balance = provider.balance().await?;
        if seen(&balance) || std::time::Instant::now() >= deadline {
            return Ok(balance);
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("=== Midnight Shielded Swap Example (A and B trade two tokens) ===\n");

    let node_url = env_or("MIDNIGHT_NODE_URL", "ws://127.0.0.1:9944");
    let indexer_url = env_or("MIDNIGHT_INDEXER_URL", "http://127.0.0.1:8088");

    let seed_b = Seed::from_hex(SEED_B)?;

    let provider_a = MidnightProvider::new(&node_url, &indexer_url);
    let wallet = Wallet::from_seed(SEED_A)?.sync(&provider_a).await?;
    let provider_a = provider_a.with_wallet(wallet);
    let provider_b = MidnightProvider::new(&node_url, &indexer_url);
    let wallet = WalletBuilder::new(&seed_b).sync(&provider_b).await?;
    let provider_b = provider_b.with_wallet(wallet);

    // Setup: two tokens to trade. X is A's genesis shielded token; Y is a fresh
    // token A mints to B (see `mint`). Not part of the swap.
    let token_x = provider_a
        .balance()
        .await?
        .shielded
        .coins
        .first()
        .map(|c| c.token_type)
        .context("wallet A has no shielded coins (is this a fresh local devnet?)")?;
    let token_y = mint::mint_token_to(&provider_a, &seed_b, &provider_b, MINT_Y).await?;
    println!("Setup: A holds token X, B holds token Y (minted).\n");

    // Snapshot both wallets so the final check proves both tokens actually moved.
    let a_before = provider_a.balance().await?;
    let b_before = provider_b.balance().await?;
    let (a_x0, a_y0) = (
        a_before.shielded_value(token_x),
        a_before.shielded_value(token_y),
    );
    let (b_x0, b_y0) = (
        b_before.shielded_value(token_x),
        b_before.shielded_value(token_y),
    );
    println!("pre-swap:  A[X={a_x0}, Y={a_y0}]  B[X={b_x0}, Y={b_y0}]\n");

    // 1. Each party builds its fee-less, unbalanced half. The two are mirrors:
    //    A gives DX of X for DY of Y; B gives DY of Y for DX of X.
    let a_half = provider_a.shielded_swap(token_x, DX, token_y, DY).await?;
    let b_half = provider_b.shielded_swap(token_y, DY, token_x, DX).await?;
    println!("1. Both halves built (A: give {DX} X take {DY} Y; B: the mirror).");

    // 2. Merge the mirrors into one balanced, fee-less transaction.
    let merged = provider_a.merge_transactions(&[a_half.into_bytes(), b_half.into_bytes()])?;
    println!("2. Merged into one balanced transaction.");

    // 3. A sponsors the merged swap's Dust fees and submits.
    let sponsored = provider_a.balance_transaction(&merged).await?;
    let pending = provider_a.submit(&sponsored).await?;
    let (_best, pending) = pending.wait_best().await?;
    let (finalized, _) = pending.wait_finalized().await?;
    println!(
        "3. Sponsored and finalized in {}.\n",
        hex::encode(finalized.block_hash)
    );

    // Both wallets resync and the balances reflect the exchange.
    let a_after = resync_until(&provider_a, |b| b.shielded_value(token_y) != a_y0).await?;
    let b_after = resync_until(&provider_b, |b| b.shielded_value(token_x) != b_x0).await?;
    let (a_x1, a_y1) = (
        a_after.shielded_value(token_x),
        a_after.shielded_value(token_y),
    );
    let (b_x1, b_y1) = (
        b_after.shielded_value(token_x),
        b_after.shielded_value(token_y),
    );
    println!("post-swap: A[X={a_x1}, Y={a_y1}]  B[X={b_x1}, Y={b_y1}]");

    let expect = |label: &str, got: u128, want: u128| -> anyhow::Result<()> {
        ensure!(got == want, "{label}: expected {want}, got {got}");
        Ok(())
    };
    expect("A's X", a_x1, a_x0 - DX)?;
    expect("A's Y", a_y1, a_y0 + DY)?;
    expect("B's X", b_x1, b_x0 + DX)?;
    expect("B's Y", b_y1, b_y0 - DY)?;

    println!("\n=== Done: A traded {DX} X for {DY} Y, B did the mirror ===");
    Ok(())
}
