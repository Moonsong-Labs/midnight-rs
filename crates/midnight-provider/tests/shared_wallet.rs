//! Two providers on one wallet share one reservation set.
//!
//! A build reserves the inputs it draws inside the wallet that drew them. Two
//! separately synced wallets on one seed each keep their own set, so the second
//! re-selects what the first already spent and the node rejects it: ledger
//! custom error 195, `InputNotInUtxos`, for the tNIGHT UTXO this test spends,
//! or 196, `DustDoubleSpend`, when the collision is on a Dust note. Sharing the
//! wallet removes the race rather than waiting it out.
//!
//! Gated on a running devnet (`MIDNIGHT_NODE_URL`, `MIDNIGHT_INDEXER_URL`).

use midnight_provider::{MidnightProvider, NIGHT, Network, Seed, WalletSeed};

const DEV_WALLET_SEED: &str = "0000000000000000000000000000000000000000000000000000000000000001";

fn devnet_urls() -> Option<(String, String)> {
    Some((
        std::env::var("MIDNIGHT_NODE_URL").ok()?,
        std::env::var("MIDNIGHT_INDEXER_URL").ok()?,
    ))
}

#[tokio::test]
async fn a_second_provider_on_the_shared_wallet_picks_different_inputs() {
    let Some((node_url, indexer_url)) = devnet_urls() else {
        eprintln!("skipping: needs MIDNIGHT_NODE_URL + MIDNIGHT_INDEXER_URL");
        return;
    };
    let seed = Seed::from_hex(DEV_WALLET_SEED).expect("dev seed");
    let recipient = seed.unshielded_address(&Network::Undeployed);

    let first = MidnightProvider::new(&node_url, &indexer_url)
        .expect("provider")
        .sync_wallet(WalletSeed::from(seed), Network::Undeployed)
        .await
        .expect("sync");
    let second = MidnightProvider::new(&node_url, &indexer_url)
        .expect("provider")
        .with_shared_wallet(first.shared_wallet().expect("the first is synced"));

    // Built back to back, with nothing on chain between them: the second build
    // can only avoid the first one's inputs by seeing its reservation.
    let a = first
        .transfer_unshielded(NIGHT, 1, &recipient)
        .await
        .expect("first provider submits");
    let b = second
        .transfer_unshielded(NIGHT, 1, &recipient)
        .await
        .expect("second provider submits");

    a.wait_finalized()
        .await
        .expect("the first transfer applies");
    b.wait_finalized()
        .await
        .expect("the second transfer applies, so it drew different inputs");
}
