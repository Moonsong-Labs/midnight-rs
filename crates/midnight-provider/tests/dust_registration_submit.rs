//! A dust registration the node accepts.
//!
//! A registration can build cleanly and still be refused, so this test
//! submits and waits for the verdict. That is the only way the size and
//! dismissal-cost rule shows up: a registration spending two unshielded
//! inputs builds cleanly and is refused with
//! `FeeCalculation(OutsideTimeToDismiss)`, custom error 168.
//!
//! Needs a devnet, and a genesis wallet with tNIGHT and dust to fund the
//! fresh address this drives.

use midnight_provider::{MidnightProvider, Network, Seed};
use midnight_wallet::NIGHT;
use midnight_wallet::{Wallet, WalletBuilder};

/// Genesis wallet: holds tNIGHT and dust, and is already registered.
const FUNDER_SEED: &str = "0000000000000000000000000000000000000000000000000000000000000001";

/// Enough tNIGHT that the coin carries real generationless dust.
const SEND: u128 = 5_000_000_000_000;

/// A seed no earlier run has used, because a registration is permanent and the
/// devnet keeps its chain between runs.
fn unused_seed() -> Seed {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let mut bytes = [0u8; 32];
    bytes[16..].copy_from_slice(&nonce.to_be_bytes());
    Seed::from_bytes(&bytes).expect("a 32-byte seed")
}

/// Two unregistered tNIGHT UTXOs is the case that fails when a registration
/// spends every one of them: it builds, and the node refuses it.
#[tokio::test]
async fn a_wallet_holding_two_unregistered_utxos_can_register() {
    let (Ok(node_url), Ok(indexer_url)) = (
        std::env::var("MIDNIGHT_NODE_URL"),
        std::env::var("MIDNIGHT_INDEXER_URL"),
    ) else {
        eprintln!("skipping: needs MIDNIGHT_NODE_URL + MIDNIGHT_INDEXER_URL");
        return;
    };

    let seed = unused_seed();
    let address = seed.unshielded_address(Network::Undeployed);

    let funder = MidnightProvider::new(&node_url, &indexer_url);
    let wallet = Wallet::from_seed(FUNDER_SEED)
        .unwrap()
        .with_network(Network::Undeployed)
        .sync(&funder)
        .await
        .expect("sync the funder");
    let funder = funder.with_wallet(wallet);

    for _ in 0..2 {
        funder
            .transfer_unshielded(NIGHT, SEND, &address)
            .await
            .expect("fund the fresh address")
            .wait_finalized()
            .await
            .expect("funding finalized");
    }

    let fresh = MidnightProvider::new(&node_url, &indexer_url);
    let wallet = WalletBuilder::new(seed)
        .with_network(Network::Undeployed)
        .sync(&fresh)
        .await
        .expect("sync the fresh wallet");
    let fresh = fresh.with_wallet(wallet);

    // Finalized on chain is not yet visible through the indexer, which the
    // fresh wallet syncs from. Poll rather than assume, as below.
    let mut dust = fresh.balance().await.expect("balance").dust;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while dust.unregistered_night_utxos < 2 && std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        fresh.resync_wallet().await.expect("resync");
        dust = fresh.balance().await.expect("balance").dust;
    }
    assert_eq!(
        dust.unregistered_night_utxos, 2,
        "the fresh address must hold the two tNIGHT UTXOs this test funded"
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
        after.unregistered_night_utxos, 1,
        "a registration covers the UTXO it spends, not the ones already held"
    );
}
