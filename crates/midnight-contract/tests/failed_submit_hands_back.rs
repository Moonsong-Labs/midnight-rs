//! A deploy that the node never gets hands its inputs back to the wallet.
//!
//! The deploy builds against a synced wallet, and its submit dials a node that
//! refuses the connection. The transaction never left the process, so the
//! wallet must get the fee Dust back at once. A deploy that submits with plain
//! `submit` keeps that Dust reserved until its TTL passes, and the next
//! fee-paying build then fails with `InsufficientDust`.
//!
//! Gated on a running devnet (`MIDNIGHT_NODE_URL`, `MIDNIGHT_INDEXER_URL`).
//! Under `make test-e2e`, which sets `MIDNIGHT_E2E`, a missing URL panics. The
//! test builds from the dev seed, so it has a binary of its own.

mod common;

mod counter {
    compact_bindgen::contract!("../../devnet/contracts/counter/compiled/compiler/analyzed-ir.sexp");
}

use std::sync::Arc;

use midnight_contract::ContractError;
use midnight_provider::{MidnightProvider, ProviderError, WalletFacade};
use midnight_wallet::LocalWallet;

/// A node URL whose dial fails: nothing listens on port 1.
const REFUSING_NODE_URL: &str = "ws://127.0.0.1:1";

#[tokio::test]
async fn a_deploy_the_node_never_gets_hands_its_inputs_back() {
    let Some((node_url, indexer_url)) = common::devnet_urls() else {
        return;
    };

    // The sync reads the network and the chain pin from the devnet's node. The
    // build reads no node: its resync finds no answer for the pin, keeps the
    // state, and replays from the indexer. Only the submit needs the node.
    let devnet = MidnightProvider::new(&node_url, &indexer_url);
    let wallet = Arc::new(LocalWallet::new(
        common::synced_wallet(&devnet, common::DEV_WALLET_SEED).await,
    ));
    let provider =
        MidnightProvider::new(REFUSING_NODE_URL, &indexer_url).with_wallet(wallet.clone());

    let Err(err) = counter::Contract::deploy(&provider)
        .with_initial_state(counter::LedgerInitialState::default())
        .with_zk_config(counter::ZK_CONFIG_DIR)
        .send()
        .await
    else {
        panic!("a deploy whose node refuses the dial must fail");
    };
    assert!(
        matches!(err, ContractError::Provider(ProviderError::Rpc(_))),
        "the deploy must fail at the connect of its submit, after a build that reserved its fee: \
         {err:?}"
    );

    let dust = wallet.balance().await.dust;
    assert_eq!(
        dust.spendable_speck, dust.balance_speck,
        "the deploy kept its fee Dust reserved after a submit that never reached the node"
    );
}
