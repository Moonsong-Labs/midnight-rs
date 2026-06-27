//! Shielded mint example — deploy a token-minting contract and mint a shielded
//! coin to an *external* wallet that finds it through normal sync.
//!
//! The contract's `mint` circuit calls `mintShieldedToken(..., left(coinPK))`,
//! creating a shielded output owned by `coinPK`. By attaching the recipient's
//! `coin_public_key -> encryption_public_key` mapping with
//! `with_coin_encryption_keys` (the Rust equivalent of midnight-js's
//! `additionalCoinEncPublicKeyMappings`), the SDK adds the discovery ciphertext
//! to that output, so the recipient's wallet discovers the coin on its own — no
//! `watchFor`, no out-of-band coordination.
//!
//! ```bash
//! docker compose -f devnet/docker-compose.yml up -d   # from the repo root
//! # wait for node RPC
//! while ! curl -sf http://localhost:9944/health > /dev/null 2>&1; do sleep 2; done
//! # wait for indexer (any HTTP response means the port is serving)
//! while ! curl -s --max-time 2 http://localhost:8088 > /dev/null 2>&1; do sleep 2; done
//! cargo run -p example-shielded-mint
//! docker compose -f devnet/docker-compose.yml down
//! ```

use midnight_provider::{MidnightProvider, Network, Seed};

mod shielded_mint {
    midnight_bindgen::contract!("../../devnet/contracts/shielded-mint/compiled/contract-info.json");
}

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_string())
}

const ZK_KEYS_DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../devnet/contracts/shielded-mint/compiled"
);

/// Dev node genesis wallet seed (funded with NIGHT at genesis) — pays fees and
/// deploys the contract.
const MINTER_SEED: &str = "0000000000000000000000000000000000000000000000000000000000000001";
/// A separate wallet that will *receive* the minted coin.
const RECIPIENT_SEED: &str = "0000000000000000000000000000000000000000000000000000000000000002";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Midnight Shielded Mint Example ===\n");

    let network = Network::Undeployed;
    let node_url = env_or("MIDNIGHT_NODE_URL", "ws://127.0.0.1:9944");
    let indexer_url = env_or("MIDNIGHT_INDEXER_URL", "http://127.0.0.1:8088");

    // 0. Sync the minter wallet (deploys + pays fees).
    println!("0. Syncing minter wallet...");
    let minter_seed = Seed::from_hex(MINTER_SEED)?;
    let provider = MidnightProvider::new(&node_url, &indexer_url)?
        .sync_wallet(minter_seed, Network::Undeployed)
        .await?;
    println!("   synced.\n");

    // 1. The recipient's coin public key (coin ownership) and encryption public
    //    key (coin discovery). Here we derive them from the recipient's seed; a
    //    minter that only has the recipient's shared address string would call
    //    `midnight_wallet::parse_shielded_recipient(&address)` to get the same.
    let recipient_seed = Seed::from_hex(RECIPIENT_SEED)?;
    let recipient = recipient_seed.shielded_wallet();
    let coin_pk = recipient.coin_public_key;
    let enc_pk = recipient.enc_public_key;
    println!(
        "1. Recipient shielded address: {}\n",
        recipient_seed.shielded_address(&network)
    );

    // 2. Deploy the mint contract.
    println!("2. Deploying mint contract...");
    let contract = shielded_mint::Contract::deploy(provider)
        .with_initial_state(shielded_mint::LedgerInitialState)
        .with_zk_keys(ZK_KEYS_DIR)
        .await?;
    let address = contract.address().to_string();
    println!("   address: {address}\n");

    // 3. Mint, attaching the recipient's coin->encryption key mapping for this
    //    call. The SDK uses the mapping to add the discovery ciphertext to the
    //    output the `mint` circuit creates.
    use midnight_bindgen::Bytes;
    let domain_sep = Bytes([0x11u8; 32]);
    let value: u64 = 1000;
    let nonce = Bytes([0x22u8; 32]);
    let coin_pk_arg = shielded_mint::ZswapCoinPublicKey {
        bytes: Bytes(coin_pk.0.0),
    };

    println!("3. Minting {value} units to the recipient...");
    contract
        .circuits()
        .with_coin_encryption_keys([(coin_pk, enc_pk)])
        .mint(domain_sep, value, nonce, coin_pk_arg)
        .await?;
    println!("   minted.\n");

    // 4. The recipient syncs from scratch and finds the coin — no watchFor.
    println!("4. Recipient syncing to discover the coin...");
    let recipient_provider = MidnightProvider::new(&node_url, &indexer_url)?
        .sync_wallet(recipient_seed, Network::Undeployed)
        .await?;
    let balance = recipient_provider.balance().await?;

    let minted = balance
        .shielded
        .coins
        .iter()
        .find(|c| c.value == value as u128);
    match minted {
        Some(coin) => {
            println!(
                "   discovered minted coin: token_type={}, value={}",
                hex::encode(coin.token_type.0.0),
                coin.value
            );
            println!("\n=== Done: recipient found the coin through normal sync ===");
        }
        None => {
            return Err(format!(
                "recipient did not discover the minted coin; shielded coins seen: {:?}",
                balance
                    .shielded
                    .coins
                    .iter()
                    .map(|c| (hex::encode(c.token_type.0.0), c.value))
                    .collect::<Vec<_>>()
            )
            .into());
        }
    }

    Ok(())
}
