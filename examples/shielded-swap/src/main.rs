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

use midnight_provider::{
    MidnightProvider, Network, Seed, ShieldedCoinBalance, ShieldedTokenType, Verdict,
};

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

/// Total spendable value of one shielded token in a balance's coin set.
fn shielded_total(coins: &[ShieldedCoinBalance], token: ShieldedTokenType) -> u128 {
    coins
        .iter()
        .filter(|c| c.token_type == token)
        .map(|c| c.value)
        .sum()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Midnight Shielded Swap Example (A and B trade two tokens) ===\n");

    let network = Network::Undeployed;
    let node_url = env_or("MIDNIGHT_NODE_URL", "ws://127.0.0.1:9944");
    let indexer_url = env_or("MIDNIGHT_INDEXER_URL", "http://127.0.0.1:8088");

    let seed_a = Seed::from_hex(SEED_A)?;
    let seed_b = Seed::from_hex(SEED_B)?;

    let provider_a = MidnightProvider::new(&node_url, &indexer_url)?
        .sync_wallet(seed_a.clone(), &network)
        .await?;
    let provider_b = MidnightProvider::new(&node_url, &indexer_url)?
        .sync_wallet(seed_b.clone(), &network)
        .await?;

    // Setup: two tokens to trade. X is A's genesis shielded token; Y is a fresh
    // token A mints to B (see `mint`). Not part of the swap.
    let token_x = provider_a
        .balance()
        .await?
        .shielded
        .coins
        .first()
        .map(|c| c.token_type)
        .ok_or("wallet A has no shielded coins (is this a fresh local devnet?)")?;
    let token_y = mint::mint_token_to(&provider_a, &seed_b, &provider_b, MINT_Y).await?;
    println!("Setup: A holds token X, B holds token Y (minted).\n");

    // Snapshot both wallets so the final check proves both tokens actually moved.
    let a_before = provider_a.balance().await?.shielded.coins;
    let b_before = provider_b.balance().await?.shielded.coins;
    let (a_x0, a_y0) = (
        shielded_total(&a_before, token_x),
        shielded_total(&a_before, token_y),
    );
    let (b_x0, b_y0) = (
        shielded_total(&b_before, token_x),
        shielded_total(&b_before, token_y),
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

    // `wait_finalized` returns Ok for any included transaction regardless of
    // outcome, so assert the verdict explicitly: a swap whose offer was dropped
    // during merge/balance would otherwise finalize as a green no-op.
    if finalized.verdict != Verdict::Success {
        return Err(format!("swap did not succeed: {:?}", finalized.verdict).into());
    }

    // Both wallets resync and the balances reflect the exchange.
    provider_a.resync_wallet().await?;
    provider_b.resync_wallet().await?;
    let a_after = provider_a.balance().await?.shielded.coins;
    let b_after = provider_b.balance().await?.shielded.coins;
    let (a_x1, a_y1) = (
        shielded_total(&a_after, token_x),
        shielded_total(&a_after, token_y),
    );
    let (b_x1, b_y1) = (
        shielded_total(&b_after, token_x),
        shielded_total(&b_after, token_y),
    );
    println!("post-swap: A[X={a_x1}, Y={a_y1}]  B[X={b_x1}, Y={b_y1}]");

    let expect = |label: &str, got: u128, want: u128| -> Result<(), String> {
        if got != want {
            return Err(format!("{label}: expected {want}, got {got}"));
        }
        Ok(())
    };
    expect("A's X", a_x1, a_x0 - DX)?;
    expect("A's Y", a_y1, a_y0 + DY)?;
    expect("B's X", b_x1, b_x0 + DX)?;
    expect("B's Y", b_y1, b_y0 - DY)?;

    println!("\n=== Done: A traded {DX} X for {DY} Y, B did the mirror ===");
    Ok(())
}
