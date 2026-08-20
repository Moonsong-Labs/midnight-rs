//! Sharing a wallet is what stops two consumers selecting the same input.
//!
//! A build reserves the inputs it draws inside the wallet that drew them, so a
//! later build skips them. Both halves of that are checked here, and neither
//! submits anything: what matters is which inputs each build *selects*, which
//! `TransferResult::spent_unshielded_inputs` reports directly. Asserting on the
//! node's rejection instead would prove the same thing more slowly, and would
//! pin this test to an upstream error code.
//!
//! Gated on a running devnet (`MIDNIGHT_NODE_URL`, `MIDNIGHT_INDEXER_URL`), the
//! same way every other devnet test in this crate is.

use midnight_provider::{
    MidnightProvider, NIGHT, Network, Seed, SharedWallet, SpentUtxoKey, Wallet, WalletSeed,
};

const DEV_WALLET_SEED: &str = "0000000000000000000000000000000000000000000000000000000000000001";

fn devnet_urls() -> Option<(String, String)> {
    Some((
        std::env::var("MIDNIGHT_NODE_URL").ok()?,
        std::env::var("MIDNIGHT_INDEXER_URL").ok()?,
    ))
}

fn overlaps(left: &[SpentUtxoKey], right: &[SpentUtxoKey]) -> bool {
    left.iter().any(|key| right.contains(key))
}

#[tokio::test]
async fn one_shared_wallet_stops_the_second_build_re_selecting() {
    let Some((node_url, indexer_url)) = devnet_urls() else {
        eprintln!("skipping: needs MIDNIGHT_NODE_URL + MIDNIGHT_INDEXER_URL");
        return;
    };
    let seed = Seed::from_hex(DEV_WALLET_SEED).expect("dev seed");
    let recipient = seed.unshielded_address(&Network::Undeployed);
    let wallet_seed = || WalletSeed::from(seed.clone());

    // The caller owns the wallet and decides that two providers drive it.
    let shared = SharedWallet::from(
        Wallet::sync(&indexer_url, wallet_seed(), Network::Undeployed)
            .await
            .expect("sync"),
    );

    // Two builds can only draw different inputs if there are two to draw.
    let night = shared
        .read()
        .await
        .unshielded_utxos()
        .iter()
        .filter(|utxo| utxo.is_night())
        .count();
    if night < 2 {
        eprintln!(
            "skipping: the wallet holds {night} tNIGHT UTXOs, so a second build has no choice"
        );
        return;
    }

    let first = MidnightProvider::new(&node_url, &indexer_url)
        .expect("provider")
        .with_wallet(shared.clone());
    let second = MidnightProvider::new(&node_url, &indexer_url)
        .expect("provider")
        .with_wallet(shared);

    // Neither is submitted, so nothing on chain separates them: the reservation
    // in the wallet they share is the only thing the second build can react to.
    let a = first
        .transfer_unshielded(NIGHT, 1, &recipient)
        .build()
        .await
        .expect("the first build");
    let b = second
        .transfer_unshielded(NIGHT, 1, &recipient)
        .build()
        .await
        .expect("the second build");

    assert!(
        !overlaps(&a.spent_unshielded_inputs, &b.spent_unshielded_inputs),
        "the second build must skip what the first reserved"
    );

    // Hand the inputs back rather than leaving them reserved until their TTL:
    // neither transaction will ever reach the chain.
    first.release_pending(&a).await.expect("release the first");
    second
        .release_pending(&b)
        .await
        .expect("release the second");
}

/// The other half: without sharing there is nothing to react to, and the second
/// build re-selects what the first one took. This is the failure the shared
/// handle exists to remove.
#[tokio::test]
async fn two_separately_synced_wallets_re_select_the_same_input() {
    let Some((node_url, indexer_url)) = devnet_urls() else {
        eprintln!("skipping: needs MIDNIGHT_NODE_URL + MIDNIGHT_INDEXER_URL");
        return;
    };
    let seed = Seed::from_hex(DEV_WALLET_SEED).expect("dev seed");
    let recipient = seed.unshielded_address(&Network::Undeployed);
    let wallet_seed = || WalletSeed::from(seed.clone());

    let first = MidnightProvider::new(&node_url, &indexer_url)
        .expect("provider")
        .sync_wallet(wallet_seed(), Network::Undeployed)
        .await
        .expect("sync the first wallet");
    let second = MidnightProvider::new(&node_url, &indexer_url)
        .expect("provider")
        .sync_wallet(wallet_seed(), Network::Undeployed)
        .await
        .expect("sync the second wallet");

    let a = first
        .transfer_unshielded(NIGHT, 1, &recipient)
        .build()
        .await
        .expect("the first build");
    let b = second
        .transfer_unshielded(NIGHT, 1, &recipient)
        .build()
        .await
        .expect("the second build");

    assert!(
        overlaps(&a.spent_unshielded_inputs, &b.spent_unshielded_inputs),
        "a wallet that cannot see the other's reservation re-selects its input"
    );

    first.release_pending(&a).await.expect("release the first");
    second
        .release_pending(&b)
        .await
        .expect("release the second");
}
