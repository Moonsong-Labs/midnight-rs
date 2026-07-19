//! Shielded swap example: a native two-party cross-token token swap.
//!
//! Two wallets exchange two different shielded tokens atomically, with neither
//! trusting the other and neither needing to pay fees themselves. Each side
//! builds one *half* of the swap: a proven, fee-less Zswap transaction that is
//! net unbalanced (gives one token, takes the other). The two halves are exact
//! mirrors, so merging them cancels both tokens into a balanced transaction a
//! sponsor funds and submits.
//!
//! Roles:
//! - A (seed 1, genesis-funded with NIGHT + Dust + a native shielded token X):
//!   deploys a mint contract to give B a second token Y, builds its own swap
//!   half, and sponsors the merged swap's Dust fees.
//! - B (seed 2, no Dust): holds token Y, builds the mirror half. Pays nothing.
//!
//! The swap: A gives `DX` of X and receives `DY` of Y; B gives `DY` of Y and
//! receives `DX` of X.
//!
//! The pieces this demonstrates:
//! - `provider.shielded_swap(give_token, give_amount, receive_token, receive_amount)`:
//!   build one fee-less, unbalanced swap half as a `DustlessTransaction`.
//! - `provider.merge_transactions(&[..])`: fold the two mirrored halves into
//!   one balanced transaction (the mirror deltas cancel, so no token deficit).
//! - `provider.balance_transaction(bytes)`: a sponsor pays the merged swap's
//!   Dust fees (the merged-swap case `balance_transaction` now covers).
//!
//! ```bash
//! docker compose -f devnet/docker-compose.yml up -d   # from the repo root
//! while ! curl -sf http://localhost:9944/health > /dev/null 2>&1; do sleep 2; done
//! while ! curl -s --max-time 2 http://localhost:8088 > /dev/null 2>&1; do sleep 2; done
//! cargo run -p example-shielded-swap
//! docker compose -f devnet/docker-compose.yml down
//! ```

use midnight_provider::{
    MidnightProvider, Network, Seed, ShieldedCoinBalance, ShieldedTokenType, Verdict,
};

mod shielded_mint {
    // Shared mint contract (see devnet/contracts/shielded-mint); used here only
    // to hand B a second token type so there is something to swap for.
    compact_bindgen::contract!("../../devnet/contracts/shielded-mint/compiled/contract-info.json");
}

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_string())
}

const ZK_KEYS_DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../devnet/contracts/shielded-mint/compiled"
);

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

    println!("0. Syncing both wallets...");
    let provider_a = MidnightProvider::new(&node_url, &indexer_url)?
        .sync_wallet(seed_a.clone(), &network)
        .await?;
    let provider_b = MidnightProvider::new(&node_url, &indexer_url)?
        .sync_wallet(seed_b.clone(), &network)
        .await?;
    println!("   A and B synced.\n");

    // Token X is A's native genesis shielded token; A swaps `DX` of it for `DY`
    // of a second token Y that B will hold.
    let token_x = provider_a
        .balance()
        .await?
        .shielded
        .coins
        .first()
        .map(|c| c.token_type)
        .ok_or("wallet A has no shielded coins (is this a fresh local devnet?)")?;
    println!(
        "1. Token X (A's native shielded): {}\n",
        hex::encode(token_x.0.0)
    );

    // --- A mints token Y to B (B needs a second token to swap, and no Dust) ---
    println!("2. A deploys a mint contract and mints token Y to B...");
    let pending = shielded_mint::Contract::deploy(&provider_a)
        .with_initial_state(shielded_mint::LedgerInitialState)
        .with_zk_config(ZK_KEYS_DIR)
        .send()
        .await?;
    let (_best, pending) = pending.wait_best().await?;
    let mint = pending.into_contract().await?;

    let b_shielded = seed_b.shielded_wallet();
    let coin_pk = b_shielded.coin_public_key;
    let enc_pk = b_shielded.enc_public_key;

    use compact_bindgen::Bytes;
    use rand::Rng;
    let domain_sep = Bytes([0x22u8; 32]);
    // Fresh nonce per run so the minted coin (and this example) stays re-runnable.
    let nonce = Bytes(rand::thread_rng().r#gen::<[u8; 32]>());
    let coin_pk_arg = shielded_mint::ZswapCoinPublicKey {
        bytes: Bytes(coin_pk.0.0),
    };
    mint.circuits()
        .with_coin_encryption_keys([(coin_pk, enc_pk)])
        .mint(domain_sep, MINT_Y, nonce, coin_pk_arg)
        .await?;
    println!("   minted {MINT_Y} of Y to B.\n");

    // B discovers the minted coin through normal sync; that is token Y.
    provider_b.resync_wallet().await?;
    let b_coins = provider_b.balance().await?.shielded.coins;
    let token_y = b_coins
        .iter()
        .find(|c| c.value == MINT_Y as u128)
        .map(|c| c.token_type)
        .ok_or("B did not discover the minted token Y")?;
    println!("3. Token Y (minted to B): {}\n", hex::encode(token_y.0.0));

    // Pre-swap balances, to prove the exchange actually moved both tokens.
    let a_before = provider_a.balance().await?.shielded.coins;
    let a_x_before = shielded_total(&a_before, token_x);
    let a_y_before = shielded_total(&a_before, token_y);
    let b_x_before = shielded_total(&b_coins, token_x);
    let b_y_before = shielded_total(&b_coins, token_y);
    println!("   pre-swap: A[X={a_x_before}, Y={a_y_before}]  B[X={b_x_before}, Y={b_y_before}]\n");

    // --- Each side builds its fee-less, unbalanced swap half ---
    println!("4. A builds its half (give {DX} X, receive {DY} Y)...");
    let a_half = provider_a.shielded_swap(token_x, DX, token_y, DY).await?;
    println!(
        "   proven, fee-less half: {} bytes",
        a_half.as_bytes().len()
    );

    println!("   B builds the mirror half (give {DY} Y, receive {DX} X)...");
    let b_half = provider_b.shielded_swap(token_y, DY, token_x, DX).await?;
    println!(
        "   proven, fee-less half: {} bytes\n",
        b_half.as_bytes().len()
    );

    // --- A merges both halves and sponsors the fees ---
    println!("5. A merges the two halves and sponsors the Dust fees...");
    let merged = provider_a.merge_transactions(&[a_half.into_bytes(), b_half.into_bytes()])?;
    let sponsored = provider_a.balance_transaction(&merged).await?;
    let pending = provider_a.submit(&sponsored).await?;
    println!("   ext hash:  {}", pending.extrinsic_hash_hex());
    let (_best, pending) = pending.wait_best().await?;
    let (finalized, _) = pending.wait_finalized().await?;
    println!("   finalized in {}\n", hex::encode(finalized.block_hash));

    // `wait_finalized` returns Ok for any included transaction regardless of
    // outcome, so assert the verdict explicitly: a swap whose offer was dropped
    // during merge/balance would otherwise finalize as a green no-op.
    if finalized.verdict != Verdict::Success {
        return Err(format!("swap did not succeed: {:?}", finalized.verdict).into());
    }

    // --- Both wallets resync and the balances reflect the exchange ---
    provider_a.resync_wallet().await?;
    provider_b.resync_wallet().await?;
    let a_after = provider_a.balance().await?.shielded.coins;
    let b_after = provider_b.balance().await?.shielded.coins;
    let a_x_after = shielded_total(&a_after, token_x);
    let a_y_after = shielded_total(&a_after, token_y);
    let b_x_after = shielded_total(&b_after, token_x);
    let b_y_after = shielded_total(&b_after, token_y);
    println!("   post-swap: A[X={a_x_after}, Y={a_y_after}]  B[X={b_x_after}, Y={b_y_after}]");

    // A gave DX of X for DY of Y; B did the mirror.
    let expect = |label: &str, got: u128, want: u128| -> Result<(), String> {
        if got != want {
            return Err(format!("{label}: expected {want}, got {got}"));
        }
        Ok(())
    };
    expect("A's X", a_x_after, a_x_before - DX)?;
    expect("A's Y", a_y_after, a_y_before + DY)?;
    expect("B's X", b_x_after, b_x_before + DX)?;
    expect("B's Y", b_y_after, b_y_before - DY)?;

    println!("\n=== Done: A traded {DX} X for {DY} Y, B did the mirror ===");
    Ok(())
}
