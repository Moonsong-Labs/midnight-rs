//! Test-token setup for the swap example, kept out of `main` so the swap flow
//! reads cleanly.
//!
//! A native two-party swap needs two distinct shielded tokens. One wallet
//! starts with only the genesis token, so before the swap we mint it a second,
//! fresh token via the shared mint contract (see devnet/contracts/shielded-mint).
//! None of this is part of the swap itself; it is just arranging something to
//! trade.

use midnight_core::contract::parse_contract_address;
use midnight_core::provider::{HashOutput, ShieldedTokenType};
use midnight_core::{MidnightProvider, Seed};

mod contract {
    midnight_core::contract!(
        "../../devnet/contracts/shielded-mint/compiled/compiler/analyzed-ir.sexp"
    );
}

/// Mint `amount` units of a fresh shielded token to `recipient`, and return
/// the type of the minted token. `minter` deploys the contract and pays the
/// fees.
///
/// The mint circuit sends the new coin to the recipient's shielded key and
/// attaches the discovery ciphertext, so the recipient finds it on resync with
/// no out-of-band coordination.
pub async fn mint_token_to(
    minter: &MidnightProvider,
    recipient: &Seed,
    amount: u64,
) -> anyhow::Result<ShieldedTokenType> {
    use midnight_core::compact_bindgen::Bytes;
    use rand::Rng;

    let mint = contract::Contract::deploy(minter)
        .with_initial_state(contract::LedgerInitialState)
        .with_zk_config(contract::ZK_CONFIG_DIR)
        .await?;

    let shielded = recipient.shielded_recipient();
    let domain_sep = [0x22u8; 32];
    // Fresh nonce per run so the coin (and the example) stays re-runnable.
    let nonce = Bytes(rand::thread_rng().r#gen::<[u8; 32]>());
    mint.circuits()
        .with_coin_encryption_keys([shielded])
        .mint(
            Bytes(domain_sep),
            amount,
            nonce,
            shielded.coin_public_key.into(),
        )
        .await?;

    Ok(parse_contract_address(mint.address())?.custom_shielded_token_type(HashOutput(domain_sep)))
}
