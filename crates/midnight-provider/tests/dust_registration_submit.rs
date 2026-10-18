//! A dust registration the node accepts, and the typed shortfalls a fresh
//! wallet meets before it registers.
//!
//! A registration can build cleanly and still be refused, so this test
//! submits and waits for the verdict. That is the only way the size and
//! dismissal-cost rule shows up: a registration spending two unshielded
//! inputs builds cleanly and is refused with
//! `FeeCalculation(OutsideTimeToDismiss)`, custom error 168.
//!
//! Needs a devnet, and a genesis wallet with tNIGHT and dust to fund the
//! fresh address this drives.

mod common;

use std::sync::Arc;
use std::time::Duration;

use midnight_provider::{MidnightProvider, Network, ProviderError, Seed};
use midnight_wallet::{
    HashOutput, LocalWallet, NIGHT, ShieldedTokenType, UnshieldedTokenType, WalletError,
};

/// Enough tNIGHT that the coin carries real generationless dust.
const SEND: u128 = 5_000_000_000_000;

/// The hex of a seed no earlier run has used, because a registration is
/// permanent and the devnet keeps its chain between runs.
fn unused_seed() -> String {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    format!("{nonce:064x}")
}

/// A fresh wallet with three unregistered tNIGHT UTXOs and no Dust gets a
/// typed shortfall for each build it cannot fund, and then registers one UTXO.
///
/// A wallet with several unregistered UTXOs is the case that fails when a
/// registration spends every one of them: it builds, and the node refuses it.
/// The shortfall steps stay in this fn, because a second fn would fund from the
/// same funder seed and race it.
#[tokio::test]
async fn a_fresh_wallet_reports_its_shortfalls_and_registers_one_of_three_utxos() {
    let Some((node_url, indexer_url)) = common::devnet_urls() else {
        return;
    };

    let seed = unused_seed();
    let keys = Seed::from_hex(&seed).expect("hex seed");
    let address = keys.unshielded_address(Network::Undeployed);
    let shielded_address = keys.shielded_address(Network::Undeployed);

    let funder = common::synced_provider(&node_url, &indexer_url, common::DEV_WALLET_SEED).await;

    for _ in 0..3 {
        funder
            .transfer_unshielded(NIGHT, SEND, &address)
            .await
            .expect("fund the fresh address")
            .wait_finalized()
            .await
            .expect("funding finalized");
    }

    let fresh = MidnightProvider::new(&node_url, &indexer_url);
    let wallet = Arc::new(LocalWallet::new(common::synced_wallet(&fresh, &seed).await));
    let fresh = Arc::new(fresh.with_wallet(Arc::clone(&wallet)));
    let sync = wallet.sync_in_background(fresh.clone(), Duration::from_secs(1));

    // The first sync can end before it sees the funding. On a timeout, the
    // assertions below read the last commit.
    if let Ok(waited) = tokio::time::timeout(
        Duration::from_secs(60),
        sync.wait_until(|w| w.balance().dust.unregistered_night_utxos >= 3),
    )
    .await
    {
        waited.expect("background sync");
    }
    let dust = fresh.balance().await.expect("balance").dust;
    assert_eq!(
        dust.unregistered_night_utxos, 3,
        "the fresh address must hold the three tNIGHT UTXOs this test funded"
    );
    assert!(
        !dust.night_generates_dust,
        "an address that has never registered must not report itself registered"
    );
    assert_eq!(
        dust.balance_speck, 0,
        "a freshly funded address holds no dust, so the registration has to \
         self-fund from generationless availability"
    );

    // Each shortfall fails before the build reserves anything, so the
    // registration below still sees all three UTXOs.
    let result = fresh
        .transfer_unshielded(NIGHT, 4 * SEND, &address)
        .build()
        .await;
    let Err(ProviderError::Wallet(WalletError::InsufficientUnshielded {
        token_type,
        required,
        available,
    })) = result
    else {
        panic!("expected InsufficientUnshielded, got {result:?}");
    };
    assert_eq!(token_type, NIGHT);
    assert_eq!(required, 4 * SEND);
    assert_eq!(
        available,
        3 * SEND,
        "the wallet can spend the three tNIGHT UTXOs it holds"
    );

    let unheld = UnshieldedTokenType(HashOutput([1; 32]));
    let result = fresh.transfer_unshielded(unheld, 1, &address).build().await;
    let Err(ProviderError::Wallet(WalletError::InsufficientUnshielded {
        token_type,
        required,
        available,
    })) = result
    else {
        panic!("expected InsufficientUnshielded, got {result:?}");
    };
    assert_eq!(token_type, unheld);
    assert_eq!(required, 1);
    assert_eq!(
        available, 0,
        "the tNIGHT UTXOs the wallet holds are not of this token"
    );

    let result = fresh
        .transfer_shielded(ShieldedTokenType(HashOutput([0; 32])), 1, &shielded_address)
        .build()
        .await;
    let Err(ProviderError::Wallet(WalletError::InsufficientShielded {
        required,
        available,
        ..
    })) = result
    else {
        panic!("expected InsufficientShielded, got {result:?}");
    };
    assert_eq!(required, 1);
    assert_eq!(available, 0, "a fresh wallet holds no shielded coins");

    let result = fresh.transfer_unshielded(NIGHT, 1, &address).build().await;
    let Err(ProviderError::Wallet(WalletError::InsufficientDust {
        required,
        available,
    })) = result
    else {
        panic!("expected InsufficientDust, got {result:?}");
    };
    assert!(required > 0, "a priced fee needs Dust");
    assert_eq!(available, 0, "an unregistered wallet holds no Dust");

    fresh
        .register_dust(None)
        .await
        .expect("build and submit the registration")
        .wait_finalized()
        .await
        .expect("the node must accept a registration that spends one tNIGHT input");

    // The wallet sees the registration only through a commit. On a timeout,
    // the assertions below read the last commit.
    if let Ok(waited) = tokio::time::timeout(
        Duration::from_secs(60),
        sync.wait_until(|w| {
            let dust = w.balance().dust;
            dust.night_generates_dust && dust.spendable_utxos > 0
        }),
    )
    .await
    {
        waited.expect("background sync");
    }
    let after = fresh.balance().await.expect("balance").dust;

    assert!(
        after.night_generates_dust,
        "the address must generate dust once the registration is on chain"
    );
    // The indexer marks the UTXO registered whatever Dust key the registration
    // names. The wallet's Dust state keeps only outputs owned by its own key.
    assert!(
        after.spendable_utxos > 0,
        "the registration must name the Dust key the wallet tracks"
    );
    assert_eq!(
        after.unregistered_night_utxos, 2,
        "a registration covers the UTXO it spends, not the ones already held"
    );
}
