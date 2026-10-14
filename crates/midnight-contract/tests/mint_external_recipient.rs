//! Live devnet E2E for full shielded mint.
//!
//! Mints a shielded coin to an EXTERNAL recipient (a second wallet) via the
//! `mint` circuit + per-call coin encryption keys, then asserts the recipient's
//! wallet discovers the coin through normal sync, no `watchFor`.
//!
//! Gated on `MIDNIGHT_NODE_URL` / `MIDNIGHT_INDEXER_URL` (a running devnet +
//! indexer).

mod shielded_mint {
    compact_bindgen::contract!(
        "../../devnet/contracts/shielded-mint/compiled/compiler/analyzed-ir.sexp"
    );
}

use compact_bindgen::Bytes;
use midnight_wallet::{Seed, Wallet, WalletBuilder};

const ZK_KEYS_DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../devnet/contracts/shielded-mint/compiled"
);

#[tokio::test]
async fn mint_to_external_recipient_discovered_by_sync() {
    let (node_url, indexer_url) = match (
        std::env::var("MIDNIGHT_NODE_URL").ok(),
        std::env::var("MIDNIGHT_INDEXER_URL").ok(),
    ) {
        (Some(n), Some(i)) => (n, i),
        _ => {
            eprintln!("skipping: needs MIDNIGHT_NODE_URL + MIDNIGHT_INDEXER_URL");
            return;
        }
    };

    // --- Recipient (cpk, epk) from a second seed (derived, never synced for
    //     the mint itself) ---
    let recip_seed =
        Seed::from_hex("0000000000000000000000000000000000000000000000000000000000000002").unwrap();
    let recip = recip_seed.shielded_recipient();
    let cpk = recip.coin_public_key;
    let epk = recip.enc_public_key;

    // --- Funder wallet deploys the mint contract ---
    let provider = midnight_provider::MidnightProvider::new(&node_url, &indexer_url);
    let wallet =
        Wallet::from_seed("0000000000000000000000000000000000000000000000000000000000000001")
            .unwrap()
            .with_network(midnight_provider::Network::Undeployed)
            .sync(&provider)
            .await
            .expect("funder sync");
    let provider = provider.with_wallet(wallet);

    let contract = shielded_mint::Contract::deploy(provider)
        .with_initial_state(shielded_mint::LedgerInitialState)
        .with_zk_config(ZK_KEYS_DIR)
        .await
        .expect("deploy mint contract");
    let address = contract.address().to_string();
    eprintln!("deployed mint contract at {address}");

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

    // The coin→enc key mapping is supplied per-call so the SDK attaches the
    // discovery ciphertext to the circuit-created output.
    contract
        .circuits()
        .with_coin_encryption_keys([(cpk, epk)])
        .mint(
            Bytes(domain_sep),
            mint_value,
            Bytes(nonce),
            shielded_mint::ZswapCoinPublicKey {
                bytes: Bytes(cpk.0.0),
            },
        )
        .await
        .expect("mint call");
    eprintln!("mint call submitted");

    // Expected custom shielded token type = tokenType(domain_sep, contract_addr).
    // The contract address is a 32-byte hex string.
    let addr_bytes = {
        let hex = address.strip_prefix("0x").unwrap_or(&address);
        let v = hex::decode(hex).expect("address hex");
        let mut a = [0u8; 32];
        a.copy_from_slice(&v);
        a
    };
    let contract_addr =
        midnight_contract::ContractAddress(midnight_base_crypto::hash::HashOutput(addr_bytes));
    let expected_tt = contract_addr
        .custom_shielded_token_type(midnight_base_crypto::hash::HashOutput(domain_sep));

    // --- Recipient syncs normally and must discover the minted coin ---
    let recip_provider = midnight_provider::MidnightProvider::new(&node_url, &indexer_url);
    let wallet = WalletBuilder::new(&recip_seed)
        .with_network(midnight_provider::Network::Undeployed)
        .sync(&recip_provider)
        .await
        .expect("recipient sync");
    let recip_provider = recip_provider.with_wallet(wallet);

    // The call above returns once the chain finalized the mint, which is
    // before the indexer has served its zswap event. A sync ends at the
    // indexer's current tip, so read once and the coin may not be there yet.
    // There is no way to wait for the indexer to reach a given block (#164),
    // so resync until it appears.
    let mut coins = Vec::new();
    for _ in 0..60 {
        recip_provider
            .resync_wallet()
            .await
            .expect("recipient resync");
        coins = recip_provider
            .balance()
            .await
            .expect("recipient balance")
            .shielded
            .coins;
        if coins
            .iter()
            .any(|c| c.token_type == expected_tt && c.value == u128::from(mint_value))
        {
            eprintln!("recipient discovered the minted coin via sync ✓");
            return;
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }

    panic!(
        "recipient wallet did not discover the minted coin (type {expected_tt}, \
         value {mint_value}); shielded coins seen: {:?}",
        coins
            .iter()
            .map(|c| (c.token_type_hex(), c.value))
            .collect::<Vec<_>>()
    );
}
