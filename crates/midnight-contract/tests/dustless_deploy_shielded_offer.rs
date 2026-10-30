//! A deploy that carries a shielded offer, which a wallet with no Dust builds
//! and a sponsor pays for, applies on chain.
//!
//! The deploy spends the deployer's coin and proves with no fee. The sponsor
//! then merges a fee intent of its own. Only the node can say that it accepts
//! a transaction of that shape.
//!
//! Gated on a running devnet (`MIDNIGHT_NODE_URL`, `MIDNIGHT_INDEXER_URL`).
//! Under `make test-e2e`, which sets `MIDNIGHT_E2E`, a missing URL panics. The
//! sponsor pays from the dev seed, so the test has a binary of its own. Two
//! tests that submit from one seed in one binary can select the same Dust.

mod common;

mod counter {
    compact_bindgen::contract!("../../devnet/contracts/counter/compiled/compiler/analyzed-ir.sexp");
}

use midnight_contract::{ShieldedOffer, ShieldedPayment, ShieldedTokenType};
use midnight_provider::MidnightProvider;

/// The value of the coin that the sponsor sends to the deployer.
const FUNDING: u128 = 10;

/// The deployer holds one shielded coin and no Dust. So its build succeeds
/// only when the deployer pays no fee. The chain must then apply the deploy
/// that the sponsor paid for.
#[tokio::test]
async fn a_wallet_with_no_dust_deploys_with_a_shielded_offer_and_a_sponsor_pays_the_fee() {
    let Some((node_url, indexer_url)) = common::devnet_urls() else {
        return;
    };

    let sponsor = MidnightProvider::new(&node_url);
    let wallet = common::synced_wallet(&sponsor, &indexer_url, common::DEV_WALLET_SEED).await;
    let sponsor = sponsor.with_wallet(wallet);

    let deployer = MidnightProvider::new(&node_url);
    let wallet = common::synced_wallet(&deployer, &indexer_url, &common::unused_seed()).await;
    let deployer_address = wallet.shielded_address();
    let deployer_recipient = wallet.seed().shielded_recipient();
    let deployer = deployer.with_wallet(wallet);

    // The dev wallet holds the dev devnet's default shielded token id
    // ([0; 32]) from genesis.
    let token_type = ShieldedTokenType(midnight_types::HashOutput([0u8; 32]));
    sponsor
        .transfer_shielded(token_type, FUNDING, &deployer_address)
        .await
        .expect("send the deployer a shielded coin")
        .wait_finalized()
        .await
        .expect("the chain must apply the transfer to the deployer");
    let coin =
        common::resync_until_coin(&deployer, token_type, FUNDING, "the coin of the sponsor").await;
    assert_eq!(
        deployer
            .balance()
            .await
            .expect("balance")
            .dust
            .balance_speck,
        0,
        "the deployer must hold no Dust"
    );

    let offer = ShieldedOffer {
        coins: vec![coin],
        outputs: vec![ShieldedPayment {
            recipient: deployer_recipient,
            token_type,
            value: 1,
        }],
    };
    let dustless = counter::Contract::deploy(&deployer)
        .with_initial_state(counter::LedgerInitialState::default())
        .with_zk_config(counter::ZK_CONFIG_DIR)
        .with_shielded_offer(offer)
        .without_dust()
        .await
        .expect("build the deploy and its offer with no Dust fee");

    let funded = sponsor
        .balance_transaction(dustless.transaction.as_bytes())
        .await
        .expect("the sponsor pays the deploy's fee");
    sponsor
        .submit_reserved(funded)
        .await
        .expect("submit the sponsored deploy")
        .wait_finalized()
        .await
        .expect("the chain must apply a sponsored deploy that carries a shielded offer");
}
