//! Test-token setup for the swap example, kept out of `main` so the swap flow
//! reads cleanly.
//!
//! A native two-party swap needs two distinct shielded tokens. One wallet
//! starts with only the genesis token, so before the swap we mint it a second,
//! fresh token via the shared mint contract (see devnet/contracts/shielded-mint).
//! None of this is part of the swap itself; it is just arranging something to
//! trade.

use anyhow::Context;
use midnight_core::provider::{ShieldedCoinBalance, ShieldedTokenType};
use midnight_core::{MidnightProvider, Seed};

mod contract {
    midnight_core::contract!(
        "../../devnet/contracts/shielded-mint/compiled/compiler/analyzed-ir.sexp"
    );
}

const ZK_KEYS_DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../devnet/contracts/shielded-mint/compiled"
);

/// Mint `amount` units of a fresh shielded token to `recipient` and wait for
/// `recipient_provider` to discover the coin through normal sync, returning the
/// minted token's type. `minter` deploys the contract and pays the fees.
///
/// The mint circuit sends the new coin to the recipient's shielded key and
/// attaches the discovery ciphertext, so the recipient finds it on resync with
/// no out-of-band coordination.
pub async fn mint_token_to(
    minter: &MidnightProvider,
    recipient: &Seed,
    recipient_provider: &MidnightProvider,
    amount: u64,
) -> anyhow::Result<ShieldedTokenType> {
    use midnight_core::compact_bindgen::Bytes;
    use rand::Rng;

    let pending = contract::Contract::deploy(minter)
        .with_initial_state(contract::LedgerInitialState)
        .with_zk_config(ZK_KEYS_DIR)
        .send()
        .await?;
    let (_best, pending) = pending.wait_best().await?;
    let mint = pending.into_contract().await?;

    let shielded = recipient.shielded_recipient();
    let coin_pk = shielded.coin_public_key;
    let enc_pk = shielded.enc_public_key;
    let domain_sep = Bytes([0x22u8; 32]);
    // Fresh nonce per run so the coin (and the example) stays re-runnable.
    let nonce = Bytes(rand::thread_rng().r#gen::<[u8; 32]>());
    let coin_pk_arg = contract::ZswapCoinPublicKey {
        bytes: Bytes(coin_pk.0.0),
    };
    mint.circuits()
        .with_coin_encryption_keys([(coin_pk, enc_pk)])
        .mint(domain_sep, amount, nonce, coin_pk_arg)
        .await?;

    let minted = |c: &ShieldedCoinBalance| c.value == amount as u128;
    crate::resync_until(recipient_provider, |b| b.shielded.coins.iter().any(minted))
        .await?
        .shielded
        .coins
        .iter()
        .find(|c| minted(c))
        .map(|c| c.token_type)
        .context("recipient did not discover the minted token")
}
