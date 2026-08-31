//! A dust registration the node accepts.
//!
//! `dust_registration_offer` covers the shape the prover sees and stops before
//! submitting, so nothing there learns what the chain makes of it. This one
//! submits and waits for the verdict, which is the only way the size and
//! dismissal-cost rule shows up: a registration spending two unshielded
//! inputs builds cleanly and is refused with
//! `FeeCalculation(OutsideTimeToDismiss)`, custom error 231.
//!
//! Needs a devnet, and a genesis wallet with tNIGHT and dust to fund the
//! fresh address this drives.

use midnight_provider::{MidnightProvider, Network, WalletSeed};
use midnight_wallet::NIGHT;
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
    let address = midnight_wallet::address::derive_unshielded(&seed, Network::Undeployed);

    let funder = MidnightProvider::new(&node_url, &indexer_url).expect("provider");
    let wallet = Wallet::sync(
        funder.indexer_url(),
        WalletSeed::try_from_hex_str(FUNDER_SEED).unwrap(),
        Network::Undeployed,
    )
    .await
    .expect("sync the funder");
    let funder = funder.with_wallet(LocalWallet::new(wallet));

    for i in 0..2 {
        let (in_block, _) = funder
            .transfer_unshielded(NIGHT, SEND, &address)
            .await
            .expect("fund the fresh address")
            .wait_finalized()
            .await
            .expect("funding finalized");
        // Reaching a block is not applying. A funding transfer that lands and
        // fails leaves the address short, and the assertions below would then
        // blame the wallet's view rather than the transfer.
        assert_eq!(
            in_block.verdict,
            midnight_provider::Verdict::Success,
            "funding transfer {i} landed but did not apply"
        );
    }

    let fresh = MidnightProvider::new(&node_url, &indexer_url).expect("provider");
    let wallet = Wallet::sync(fresh.indexer_url(), seed, Network::Undeployed)
        .await
        .expect("sync the fresh wallet");
    let fresh = fresh.with_wallet(LocalWallet::new(wallet));

    // Finalized on chain is not yet visible through the indexer, and the
    // wallet only reports what the indexer served it. Poll for the second
    // funding UTXO rather than assume, so a slow indexer reads as slow and not
    // as a wallet that lost a UTXO.
    let mut dust = fresh.balance().await.expect("balance").dust;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while dust.unregistered_night_utxos < 2 && std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
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
