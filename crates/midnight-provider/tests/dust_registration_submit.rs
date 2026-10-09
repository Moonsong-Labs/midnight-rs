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

use midnight_provider::{MidnightProvider, Network, ProviderError, WalletSeed};
use midnight_wallet::{HashOutput, NIGHT, ShieldedTokenType, UnshieldedTokenType, WalletError};
use midnight_wallet::{LocalWallet, Wallet};

/// Genesis wallet: holds tNIGHT and dust, and is already registered.
const FUNDER_SEED: &str = "0000000000000000000000000000000000000000000000000000000000000001";

/// Enough tNIGHT that the coin carries real generationless dust.
const SEND: u128 = 5_000_000_000_000;

/// A seed no earlier run has used, because a registration is permanent and the
/// devnet keeps its chain between runs.
fn unused_seed() -> WalletSeed {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    WalletSeed::try_from_hex_str(&format!("{nonce:064x}")).expect("seed from nonce")
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
    let (Ok(node_url), Ok(indexer_url)) = (
        std::env::var("MIDNIGHT_NODE_URL"),
        std::env::var("MIDNIGHT_INDEXER_URL"),
    ) else {
        eprintln!("skipping: needs MIDNIGHT_NODE_URL + MIDNIGHT_INDEXER_URL");
        return;
    };

    let seed = unused_seed();
    let address = midnight_wallet::address::derive_unshielded(&seed, Network::Undeployed);
    let shielded_address = midnight_wallet::address::derive_shielded(&seed, Network::Undeployed);

    let funder = MidnightProvider::new(&node_url, &indexer_url).expect("provider");
    let wallet = Wallet::sync(
        &funder,
        WalletSeed::try_from_hex_str(FUNDER_SEED).unwrap(),
        Network::Undeployed,
    )
    .await
    .expect("sync the funder");
    let funder = funder.with_wallet(LocalWallet::new(wallet));

    for _ in 0..3 {
        funder
            .transfer_unshielded(NIGHT, SEND, &address)
            .await
            .expect("fund the fresh address")
            .wait_finalized()
            .await
            .expect("funding finalized");
    }

    let fresh = MidnightProvider::new(&node_url, &indexer_url).expect("provider");
    let wallet = Wallet::sync(&fresh, seed, Network::Undeployed)
        .await
        .expect("sync the fresh wallet");
    let fresh = fresh.with_wallet(LocalWallet::new(wallet));

    // Finalized on chain is not yet visible through the indexer, which the
    // fresh wallet syncs from. Poll rather than assume, as below.
    let mut dust = fresh.balance().await.expect("balance").dust;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while dust.unregistered_night_utxos < 3 && std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        fresh.resync_wallet().await.expect("resync");
        dust = fresh.balance().await.expect("balance").dust;
    }
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

    let result = fresh
        .shielded_swap(
            ShieldedTokenType(HashOutput([0; 32])),
            1,
            ShieldedTokenType(HashOutput([1; 32])),
            1,
        )
        .build()
        .await;
    let Err(ProviderError::Wallet(WalletError::InsufficientShielded {
        required,
        available,
        ..
    })) = result
    else {
        panic!("expected InsufficientShielded from the swap, got {result:?}");
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
    assert!(
        required.is_some_and(|r| r > 0),
        "a priced fee needs Dust, got {required:?}"
    );
    assert_eq!(available, 0, "an unregistered wallet holds no Dust");

    fresh
        .register_dust(None)
        .await
        .expect("build and submit the registration")
        .wait_finalized()
        .await
        .expect("the node must accept a registration that spends one tNIGHT input");

    // Finalized on chain is not yet visible through the indexer, and the
    // wallet only learns UTXO state by replaying what the indexer serves. Poll
    // rather than assume, so a slow indexer reads as slow and not as broken.
    let mut after = fresh.balance().await.expect("balance").dust;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while !after.night_generates_dust && std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        fresh.resync_wallet().await.expect("resync");
        after = fresh.balance().await.expect("balance").dust;
    }

    assert!(
        after.night_generates_dust,
        "the address must generate dust once the registration is on chain"
    );
    assert_eq!(
        after.unregistered_night_utxos, 2,
        "a registration covers the UTXO it spends, not the ones already held"
    );
}
