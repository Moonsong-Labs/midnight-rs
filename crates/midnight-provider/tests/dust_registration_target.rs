//! A Dust registration that names the Dust address of another wallet.
//!
//! Only the chain shows where the Dust of a registration goes, so this test
//! submits one and reads the Dust of both wallets. Then the other wallet pays
//! the fee of a transaction of the registering wallet with that Dust.
//!
//! Needs a devnet, and a genesis wallet with tNIGHT and dust to fund the
//! fresh address this drives.

mod common;

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use midnight_provider::{DustlessBuilder, MidnightProvider, Network, ProviderError, Seed};
use midnight_wallet::{DustBalance, NIGHT, WalletError, parse_dust_address};

/// Enough tNIGHT that the coin carries real generationless dust.
const SEND: u128 = 5_000_000_000_000;

/// The hex of two seeds no earlier run has used, because a registration is
/// permanent and the devnet keeps its chain between runs.
fn unused_seeds() -> (String, String) {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    (format!("{nonce:064x}"), format!("{:064x}", nonce + 1))
}

/// Resync `provider` until its Dust balance satisfies `done`, and return that
/// balance. Finalized on chain is not yet visible through the indexer, which
/// the wallets sync from.
async fn wait_for_dust(
    provider: &MidnightProvider,
    what: &str,
    done: impl Fn(&DustBalance) -> bool,
) -> DustBalance {
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        provider.resync_wallet().await.expect("resync");
        let dust = provider.balance().await.expect("balance").dust;
        if done(&dust) {
            return dust;
        }
        assert!(Instant::now() < deadline, "{what}, got {dust:?}");
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

/// A cold wallet registers its tNIGHT to the Dust address of a hot wallet.
/// The hot wallet gets the Dust, the cold wallet gets none, and the hot
/// wallet pays the fee of a transfer of the cold wallet with it.
#[tokio::test]
async fn a_registration_to_another_dust_address_funds_that_wallet() {
    let Some((node_url, indexer_url)) = common::devnet_urls() else {
        return;
    };

    let (cold_seed, hot_seed) = unused_seeds();
    let cold_address = Seed::from_hex(&cold_seed)
        .expect("hex seed")
        .unshielded_address(Network::Undeployed);

    let funder = common::synced_provider(&node_url, &indexer_url, common::DEV_WALLET_SEED).await;
    funder
        .transfer_unshielded(NIGHT, SEND, &cold_address)
        .await
        .expect("fund the cold address")
        .wait_finalized()
        .await
        .expect("funding finalized");

    let cold = common::synced_provider(&node_url, &indexer_url, &cold_seed).await;
    wait_for_dust(&cold, "the cold wallet must hold the funded tNIGHT", |d| {
        d.unregistered_night_utxos == 1
    })
    .await;

    let hot = MidnightProvider::new(&node_url);
    let hot_wallet = common::synced_wallet(&hot, &indexer_url, &hot_seed).await;
    let target = parse_dust_address(&hot_wallet.dust_address(), Network::Undeployed)
        .expect("the hot wallet's Dust address");
    let hot = hot.with_wallet(hot_wallet);

    cold.register_dust(None)
        .to(target)
        .await
        .expect("build and submit the registration")
        .wait_finalized()
        .await
        .expect("the node must accept a registration that names another Dust address");

    wait_for_dust(
        &hot,
        "the target must get the Dust of the registration",
        |d| d.spendable_utxos > 0,
    )
    .await;
    // The indexer marks the UTXO registered whatever Dust key the registration
    // names, so this waits for the registration to reach the cold wallet.
    let cold_dust = wait_for_dust(&cold, "the registered tNIGHT must generate Dust", |d| {
        d.night_generates_dust
    })
    .await;
    assert_eq!(
        (cold_dust.spendable_utxos, cold_dust.balance_speck),
        (0, 0),
        "the registration must not name the cold wallet's own Dust key"
    );

    let dustless = cold
        .transfer_unshielded(NIGHT, SEND, &cold_address)
        .without_dust()
        .await
        .expect("build the fee-less transfer");
    // The hot wallet values its Dust at the time of its last Dust event, so
    // the first attempt can fall short.
    let deadline = Instant::now() + Duration::from_secs(120);
    let funded = loop {
        match hot.balance_transaction(dustless.as_bytes()).await {
            Ok(funded) => break funded,
            Err(ProviderError::Wallet(WalletError::InsufficientDust { .. }))
                if Instant::now() < deadline =>
            {
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            Err(err) => panic!("the hot wallet must pay the fee with the Dust it got: {err}"),
        }
    };
    hot.submit_reserved(funded)
        .await
        .expect("submit the sponsored transfer")
        .wait_finalized()
        .await
        .expect("the node must accept a fee paid with the Dust of another wallet's tNIGHT");
}
