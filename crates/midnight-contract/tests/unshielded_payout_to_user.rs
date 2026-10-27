//! A contract can pay an unshielded token to a user address.
//!
//! The contract mints an unshielded token to itself, then sends some of it to a
//! user. The send claims an unshielded spend in the call's transcript, and
//! verification requires a real unshielded offer to cover every claimed one, so
//! the call builds one from the transcript. Without it the node refuses the
//! transaction with `EffectsCheckFailure`.
//!
//! The recipient is a second wallet, and the assertion is that its balance
//! grew. Paying the caller's own address would pass even if every payout were
//! wrongly owned by the caller, which is the distinction `PayoutOutput` exists
//! for; asserting only that the call returned would pass on a transaction that
//! paid nobody.
//!
//! Gated on a running devnet (`MIDNIGHT_NODE_URL`, `MIDNIGHT_INDEXER_URL`).

mod common;

mod unshielded_payout {
    compact_bindgen::contract!(
        "../../devnet/contracts/unshielded-payout/compiled/compiler/analyzed-ir.sexp"
    );
}

use compact_bindgen::Bytes;
use midnight_provider::MidnightProvider;
use midnight_provider::UnshieldedTokenType;

const DOMAIN_SEP: [u8; 32] = [0x33; 32];
const MINTED: u64 = 1_000;
const PAID: u128 = 10;

#[tokio::test]
async fn a_contract_pays_an_unshielded_token_to_a_user() {
    let Some((node_url, indexer_url)) = common::devnet_urls() else {
        return;
    };

    let provider = MidnightProvider::new(&node_url);
    let wallet = common::synced_wallet(&provider, &indexer_url, common::DEV_WALLET_SEED).await;
    let provider = provider.with_wallet(wallet);

    let contract = unshielded_payout::Contract::deploy(provider)
        .with_initial_state(unshielded_payout::LedgerInitialState)
        .with_zk_config(unshielded_payout::ZK_CONFIG_DIR)
        .await
        .expect("deploy");
    eprintln!("deployed at {}", contract.address());

    // The minted token is derived from the contract's own address, so it can
    // only be named once the contract exists.
    let colour = midnight_contract::parse_address(contract.address())
        .expect("contract address")
        .custom_unshielded_token_type(midnight_base_crypto::hash::HashOutput(DOMAIN_SEP));

    // Mint to the contract itself, so it has a balance to pay out of.
    contract
        .circuits()
        .mintToSelf(Bytes(DOMAIN_SEP), MINTED)
        .await
        .expect("the contract must be able to mint to itself");

    // A different wallet from the one that deploys and pays the fees, so an
    // output wrongly owned by the caller would fail this.
    let recipient_provider = MidnightProvider::new(&node_url);
    let wallet =
        common::synced_wallet(&recipient_provider, &indexer_url, common::RECIPIENT_SEED).await;
    let recipient = wallet.user_address();
    let recipient_provider = recipient_provider.with_wallet(wallet);

    let before = held(&recipient_provider, colour).await;
    eprintln!("recipient holds {before} before the payout");

    contract
        .circuits()
        .payUser(Bytes(colour.0.0), PAID, recipient.into())
        .await
        .expect("paying a user an unshielded token must be accepted");

    // The call returns once the transaction is in a block, which is before the
    // indexer has served it, so poll rather than read once.
    let after = held_until(&recipient_provider, colour, before + PAID).await;
    eprintln!("recipient holds {after} after the payout");
    assert_eq!(
        after,
        before + PAID,
        "the payout must reach the recipient, not merely be accepted"
    );
}

/// What the wallet holds of `colour` right now.
async fn held(provider: &MidnightProvider, colour: UnshieldedTokenType) -> u128 {
    provider.resync_wallet().await.expect("resync");
    provider
        .balance()
        .await
        .expect("balance")
        .unshielded_value(colour)
}

/// [`held`], retried until it reaches `target` or two minutes elapse.
///
/// There is no way to wait for the indexer to reach a given block (#164), so
/// this polls. The recipient is a second wallet, whose sync adds to the wait.
///
/// Returns whatever it last saw, so a caller's assertion reports the real
/// figure rather than a timeout.
async fn held_until(
    provider: &MidnightProvider,
    colour: UnshieldedTokenType,
    target: u128,
) -> u128 {
    let mut seen = 0;
    for _ in 0..60 {
        seen = held(provider, colour).await;
        if seen >= target {
            return seen;
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
    seen
}
