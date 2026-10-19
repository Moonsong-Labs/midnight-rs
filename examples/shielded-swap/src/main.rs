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
//! cargo run -p example-shielded-swap
//! docker compose -f devnet/docker-compose.yml down
//! ```

mod mint;

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, ensure};
use midnight_core::wallet::LocalWallet;
use midnight_core::{MidnightProvider, Seed, Wallet};

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

/// The pause between two passes of each background sync. A short pause ends a
/// wait soon after the indexer serves the transaction.
const SYNC_PAUSE: Duration = Duration::from_secs(1);

/// How long a wallet waits to see the effect of a finalized transaction.
const EFFECT_TIMEOUT: Duration = Duration::from_secs(60);

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("=== Midnight Shielded Swap Example (A and B trade two tokens) ===\n");

    let node_url = env_or("MIDNIGHT_NODE_URL", "ws://127.0.0.1:9944");
    let indexer_url = env_or("MIDNIGHT_INDEXER_URL", "http://127.0.0.1:8088");

    let seed_b = Seed::from_hex(SEED_B)?;

    let provider_a = MidnightProvider::new(&node_url, &indexer_url);
    let wallet_a = Arc::new(LocalWallet::new(
        Wallet::from_seed(SEED_A)?.sync(&provider_a).await?,
    ));
    let provider_a = Arc::new(provider_a.with_wallet(Arc::clone(&wallet_a)));
    let sync_a = wallet_a.sync_in_background(provider_a.clone(), SYNC_PAUSE);
    let provider_b = MidnightProvider::new(&node_url, &indexer_url);
    let wallet_b = Arc::new(LocalWallet::new(
        Wallet::from_seed(SEED_B)?.sync(&provider_b).await?,
    ));
    let provider_b = Arc::new(provider_b.with_wallet(Arc::clone(&wallet_b)));
    let sync_b = wallet_b.sync_in_background(provider_b.clone(), SYNC_PAUSE);

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
    let token_y = mint::mint_token_to(&provider_a, &seed_b, &sync_b, MINT_Y).await?;
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

    // Each wallet waits until its background sync sees both tokens move.
    let (a_x_want, a_y_want) = (a_x0 - DX, a_y0 + DY);
    let (b_x_want, b_y_want) = (b_x0 + DX, b_y0 - DY);
    tokio::time::timeout(
        EFFECT_TIMEOUT,
        sync_a.wait_until(|b| {
            b.shielded_value(token_x) == a_x_want && b.shielded_value(token_y) == a_y_want
        }),
    )
    .await
    .with_context(|| format!("A did not reach X={a_x_want}, Y={a_y_want}"))??;
    tokio::time::timeout(
        EFFECT_TIMEOUT,
        sync_b.wait_until(|b| {
            b.shielded_value(token_x) == b_x_want && b.shielded_value(token_y) == b_y_want
        }),
    )
    .await
    .with_context(|| format!("B did not reach X={b_x_want}, Y={b_y_want}"))??;
    let a_after = provider_a.balance().await?;
    let b_after = provider_b.balance().await?;
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
    expect("A's X", a_x1, a_x_want)?;
    expect("A's Y", a_y1, a_y_want)?;
    expect("B's X", b_x1, b_x_want)?;
    expect("B's Y", b_y1, b_y_want)?;

    println!("\n=== Done: A traded {DX} X for {DY} Y, B did the mirror ===");
    Ok(())
}
