//! A Dust deregistration that the node accepts, and that stops every tNIGHT
//! UTXO of the wallet from generating Dust in one transaction.
//!
//! The deregistration puts one UTXO in the guaranteed offer and the others in
//! the fallible offer. A registration that spends two guaranteed inputs is
//! refused with `FeeCalculation(OutsideTimeToDismiss)`, and only a submit
//! shows whether the node accepts this shape. So the test submits and waits
//! for the verdict.
//!
//! Needs a devnet, and a genesis wallet with tNIGHT and dust to fund the
//! fresh address this drives.

mod common;

use std::time::{Duration, Instant};

use midnight_provider::{MidnightProvider, Network, Seed};
use midnight_wallet::NIGHT;

/// Enough tNIGHT that the Dust it generates pays a fee within seconds.
const SEND: u128 = 5_000_000_000_000;

/// The tNIGHT UTXOs that the deregistration spends. Three puts two inputs in
/// the fallible offer, each with its own signature.
const UTXOS: usize = 3;

/// The hex of a seed no earlier run has used, because the devnet keeps its
/// chain between runs.
fn unused_seed() -> String {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    format!("{nonce:064x}")
}

/// What the wallet reads about its tNIGHT and its Dust.
#[derive(Debug)]
struct Reading {
    night_utxos: usize,
    generating: usize,
    spendable_speck: u128,
}

/// Send one tNIGHT UTXO of `SEND` from `funder` to `address`, and wait until it
/// is finalized.
async fn fund(funder: &MidnightProvider, address: &str) {
    funder
        .transfer_unshielded(NIGHT, SEND, address)
        .await
        .expect("fund the fresh address")
        .wait_finalized()
        .await
        .expect("funding finalized");
}

async fn read(provider: &MidnightProvider) -> Reading {
    let utxos = provider.unshielded_utxos().await.expect("unshielded UTXOs");
    let night: Vec<_> = utxos.iter().filter(|u| u.is_night()).collect();
    Reading {
        night_utxos: night.len(),
        generating: night.iter().filter(|u| u.is_registered_for_dust()).count(),
        spendable_speck: provider
            .balance()
            .await
            .expect("balance")
            .dust
            .spendable_speck,
    }
}

/// Resync until `done` holds for the wallet's reading. Panics with `what`
/// after a minute.
///
/// Finalized on chain is not yet visible through the indexer, and the wallet
/// only learns UTXO state by replaying what the indexer serves. Polling makes
/// a slow indexer read as slow and not as broken.
async fn wait_for(provider: &MidnightProvider, what: &str, done: impl Fn(&Reading) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        provider.resync_wallet().await.expect("resync");
        let reading = read(provider).await;
        if done(&reading) {
            return;
        }
        assert!(Instant::now() < deadline, "{what}, got {reading:?}");
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

/// A wallet whose three tNIGHT UTXOs generate Dust deregisters in one
/// transaction, and afterwards none of them generates.
///
/// The wallet registers one UTXO, and the other two reach the registered
/// address, so they generate with no further call.
#[tokio::test]
async fn one_deregistration_stops_every_tnight_utxo_from_generating() {
    let Some((node_url, indexer_url)) = common::devnet_urls() else {
        return;
    };

    let seed = unused_seed();
    let address = Seed::from_hex(&seed)
        .expect("hex seed")
        .unshielded_address(Network::Undeployed);

    let funder = common::synced_provider(&node_url, &indexer_url, common::DEV_WALLET_SEED).await;

    fund(&funder, &address).await;
    let fresh = common::synced_provider(&node_url, &indexer_url, &seed).await;
    wait_for(&fresh, "the funded tNIGHT must reach the wallet", |r| {
        r.night_utxos == 1
    })
    .await;
    fresh
        .register_dust(None)
        .await
        .expect("build and submit the registration")
        .wait_finalized()
        .await
        .expect("the registration finalized");

    for _ in 1..UTXOS {
        fund(&funder, &address).await;
    }
    wait_for(
        &fresh,
        "every tNIGHT UTXO must generate Dust, and the wallet must hold Dust for the fee",
        |r| r.night_utxos == UTXOS && r.generating == UTXOS && r.spendable_speck > 0,
    )
    .await;

    let result = fresh
        .deregister_dust()
        .build()
        .await
        .expect("build the deregistration");
    assert_eq!(
        result.spent.unshielded.len(),
        UTXOS,
        "the deregistration must reserve every tNIGHT UTXO it spends"
    );
    fresh
        .submit_reserved(result)
        .await
        .expect("submit the deregistration")
        .wait_finalized()
        .await
        .expect("the node must accept a deregistration that spends every tNIGHT UTXO");

    wait_for(
        &fresh,
        "the deregistration must create every tNIGHT UTXO again, and none may generate Dust",
        |r| r.night_utxos == UTXOS && r.generating == 0,
    )
    .await;
}
