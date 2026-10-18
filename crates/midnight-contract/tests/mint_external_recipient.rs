//! Live devnet E2E for full shielded mint.
//!
//! Mints a shielded coin to an EXTERNAL recipient (a second wallet) via the
//! `mint` circuit + per-call coin encryption keys, then asserts the recipient's
//! wallet discovers the coin through normal sync, no `watchFor`.
//!
//! Gated on `MIDNIGHT_NODE_URL` / `MIDNIGHT_INDEXER_URL` (a running devnet +
//! indexer).

mod common;

mod shielded_mint {
    compact_bindgen::contract!(
        "../../devnet/contracts/shielded-mint/compiled/compiler/analyzed-ir.sexp"
    );
}

use std::sync::Arc;
use std::time::Duration;

use compact_bindgen::Bytes;
use midnight_provider::ShieldedCoinBalance;
use midnight_wallet::{LocalWallet, Seed};

#[tokio::test]
async fn mint_to_external_recipient_discovered_by_sync() {
    let Some((node_url, indexer_url)) = common::devnet_urls() else {
        return;
    };

    // --- Recipient keys from a second seed (derived, never synced for the
    //     mint itself) ---
    let recip_seed = Seed::from_hex(common::RECIPIENT_SEED).unwrap();
    let recip = recip_seed.shielded_recipient();

    // --- Funder wallet deploys the mint contract ---
    let provider = midnight_provider::MidnightProvider::new(&node_url, &indexer_url);
    let wallet = common::synced_wallet(&provider, common::DEV_WALLET_SEED).await;
    let provider = provider.with_wallet(wallet);

    let contract = shielded_mint::Contract::deploy(provider)
        .with_initial_state(shielded_mint::LedgerInitialState)
        .with_zk_config(shielded_mint::ZK_CONFIG_DIR)
        .await
        .expect("deploy mint contract");
    eprintln!("deployed mint contract at {}", contract.address());

    // --- Call mint(domain_sep, value, nonce, coinPK) ---
    //
    // The contract wraps the recipient with a compile-time-constant
    // `left<ZswapCoinPublicKey, ContractAddress>(coinPK)`, so `coinPK` is a plain
    // `ZswapCoinPublicKey` struct (a single `Bytes<32>`), not a runtime `Either`.
    // A runtime `Either` recipient makes the `mintShieldedToken` builtin compile to
    // a fixed 80-public-input circuit that carries the (untaken) contract-recipient
    // branch; its skipped public inputs are non-zero (recipient/commitment data) and
    // can't be reproduced by the prover's zero-noop padding, which fails SNARK verify
    // with InvalidProof. The constant `left(...)` folds that branch away.
    let domain_sep = [0x11u8; 32];
    let mint_value: u64 = 1000;
    let nonce = [0x22u8; 32];

    // The recipient is supplied per call so the SDK attaches the discovery
    // ciphertext to the circuit-created output.
    contract
        .circuits()
        .with_coin_encryption_keys([recip])
        .mint(
            Bytes(domain_sep),
            mint_value,
            Bytes(nonce),
            recip.coin_public_key.into(),
        )
        .await
        .expect("mint call");
    eprintln!("mint call submitted");

    // Expected custom shielded token type = tokenType(domain_sep, contract_addr).
    let expected_tt = midnight_contract::parse_address(contract.address())
        .expect("contract address")
        .custom_shielded_token_type(midnight_base_crypto::hash::HashOutput(domain_sep));

    // --- Recipient syncs normally and must discover the minted coin ---
    let recip_provider = midnight_provider::MidnightProvider::new(&node_url, &indexer_url);
    let wallet = Arc::new(LocalWallet::new(
        common::synced_wallet(&recip_provider, common::RECIPIENT_SEED).await,
    ));
    let recip_provider = Arc::new(recip_provider.with_wallet(Arc::clone(&wallet)));
    let sync = wallet.sync_in_background(recip_provider.clone(), Duration::from_secs(1));

    // The call above returns once the chain finalized the mint, which is
    // before the indexer has served its zswap event. A sync ends at the
    // indexer's current tip, so the first sync can end before it sees the
    // mint. On a timeout, the assertion below reads the last commit.
    let minted =
        |c: &ShieldedCoinBalance| c.token_type == expected_tt && c.value == u128::from(mint_value);
    if let Ok(waited) = tokio::time::timeout(
        Duration::from_secs(120),
        sync.wait_until(|w| w.balance().shielded.coins.iter().any(minted)),
    )
    .await
    {
        waited.expect("background sync");
    }
    let coins = recip_provider
        .balance()
        .await
        .expect("recipient balance")
        .shielded
        .coins;
    assert!(
        coins.iter().any(minted),
        "recipient wallet did not discover the minted coin (type {expected_tt}, \
         value {mint_value}); shielded coins seen: {:?}",
        coins
            .iter()
            .map(|c| (c.token_type_hex(), c.value))
            .collect::<Vec<_>>()
    );
    eprintln!("recipient discovered the minted coin via sync ✓");
}
