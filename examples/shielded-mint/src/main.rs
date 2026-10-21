//! Shielded mint example — deploy a token-minting contract and mint a shielded
//! coin to an *external* wallet that finds it through normal sync.
//!
//! The contract's `mint` circuit calls `mintShieldedToken(..., left(coinPK))`,
//! creating a shielded output owned by `coinPK`. By attaching the recipient
//! with `with_coin_encryption_keys`, the SDK adds the discovery ciphertext to
//! that output, so the recipient's wallet discovers the coin on its own, no
//! `watchFor`, no out-of-band coordination.
//!
//! ```bash
//! make dev-up   # from the repo root: starts the devnet and waits for a block
//! make run-shielded-mint   # from the repo root
//! make dev-down   # from the repo root
//! ```

use std::sync::Arc;
use std::time::Duration;

use anyhow::bail;
use midnight_core::contract::parse_address;
use midnight_core::provider::{HashOutput, ShieldedCoinBalance};
use midnight_core::wallet::LocalWallet;
use midnight_core::{MidnightProvider, Seed, Wallet};

mod shielded_mint {
    midnight_core::contract!(
        "../../devnet/contracts/shielded-mint/compiled/compiler/analyzed-ir.sexp"
    );
}

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_string())
}

/// Dev node genesis wallet seed (funded with NIGHT at genesis) — pays fees and
/// deploys the contract.
const MINTER_SEED: &str = "0000000000000000000000000000000000000000000000000000000000000001";
/// A separate wallet that will *receive* the minted coin.
const RECIPIENT_SEED: &str = "0000000000000000000000000000000000000000000000000000000000000002";

/// The pause between two passes of the recipient's background sync. A short
/// pause ends the wait soon after the indexer serves the mint.
const SYNC_PAUSE: Duration = Duration::from_secs(1);

/// How long the recipient waits to see the minted coin.
const EFFECT_TIMEOUT: Duration = Duration::from_secs(60);

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("=== Midnight Shielded Mint Example ===\n");

    let node_url = env_or("MIDNIGHT_NODE_URL", "ws://127.0.0.1:9944");
    let indexer_url = env_or("MIDNIGHT_INDEXER_URL", "http://127.0.0.1:8088");

    // 0. Sync the minter wallet (deploys + pays fees).
    println!("0. Syncing minter wallet...");
    let provider = MidnightProvider::new(&node_url, &indexer_url);
    let wallet = Wallet::from_seed(MINTER_SEED)?.sync(&provider).await?;
    let provider = provider.with_wallet(wallet);
    let network = provider.network().await?;
    println!("   synced.\n");

    // 1. The recipient's coin public key (coin ownership) and encryption public
    //    key (coin discovery). Here we derive them from the recipient's seed; a
    //    minter that only has the recipient's shared address string would call
    //    `midnight_core::wallet::parse_shielded_recipient(&address, network)`
    //    to get the same.
    let recipient_seed = Seed::from_hex(RECIPIENT_SEED)?;
    let recipient = recipient_seed.shielded_recipient();
    println!(
        "1. Recipient shielded address: {}\n",
        recipient_seed.shielded_address(&network)
    );

    // 2. Deploy the mint contract.
    println!("2. Deploying mint contract...");
    let contract = shielded_mint::Contract::deploy(provider)
        .with_initial_state(shielded_mint::LedgerInitialState)
        .with_zk_config(shielded_mint::ZK_CONFIG_DIR)
        .await?;
    let address = contract.address().to_string();
    println!("   address: {address}\n");

    // 3. Mint, attaching the recipient for this call. The SDK seals the
    //    discovery ciphertext of the output the `mint` circuit creates to the
    //    recipient's encryption key.
    use midnight_core::compact_bindgen::Bytes;
    use rand::Rng;
    let domain_sep = [0x11u8; 32];
    let value: u64 = 1000;
    // A fresh nonce per mint: the coin commitment is derived from it, so a
    // unique nonce keeps each minted coin distinct (and the example re-runnable).
    let nonce = Bytes(rand::thread_rng().r#gen::<[u8; 32]>());

    println!("3. Minting {value} units to the recipient...");
    contract
        .circuits()
        .with_coin_encryption_keys([recipient])
        .mint(
            Bytes(domain_sep),
            value,
            nonce,
            recipient.coin_public_key.into(),
        )
        .await?;
    println!("   minted.\n");

    // 4. The recipient syncs from scratch and finds the coin — no watchFor.
    println!("4. Recipient syncing to discover the coin...");
    let recipient_provider = MidnightProvider::new(&node_url, &indexer_url);
    let wallet = Arc::new(LocalWallet::new(
        Wallet::from_seed(RECIPIENT_SEED)?
            .sync(&recipient_provider)
            .await?,
    ));
    let recipient_provider = Arc::new(recipient_provider.with_wallet(Arc::clone(&wallet)));
    let sync = wallet.sync_in_background(recipient_provider.clone(), SYNC_PAUSE);

    let token_type = parse_address(&address)?.custom_shielded_token_type(HashOutput(domain_sep));
    let is_minted =
        |c: &ShieldedCoinBalance| c.token_type == token_type && c.value == u128::from(value);
    // The first sync can end before it sees the mint. On a timeout, the check
    // below reads the last commit.
    if let Ok(waited) = tokio::time::timeout(
        EFFECT_TIMEOUT,
        sync.wait_until(|b| b.shielded.coins.iter().any(is_minted)),
    )
    .await
    {
        waited?;
    }
    let balance = recipient_provider.balance().await?;

    let minted = balance.shielded.coins.iter().find(|c| is_minted(c));
    match minted {
        Some(coin) => {
            println!(
                "   discovered minted coin: token_type={}, value={}",
                coin.token_type_hex(),
                coin.value
            );
            println!("\n=== Done: recipient found the coin through normal sync ===");
        }
        None => {
            bail!(
                "recipient did not discover the minted coin; last pass error: {:?}; shielded coins seen: {:?}",
                sync.last_error().map(|e| e.to_string()),
                balance
                    .shielded
                    .coins
                    .iter()
                    .map(|c| (c.token_type_hex(), c.value))
                    .collect::<Vec<_>>()
            );
        }
    }

    Ok(())
}
