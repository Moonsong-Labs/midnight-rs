//! Devnet setup for the devnet tests of this crate.

#![allow(dead_code, reason = "each test crate uses a subset of these helpers")]

use midnight_provider::{MidnightProvider, Network, Seed};
use midnight_wallet::WalletBuilder;

/// The seed of the devnet genesis wallet, which holds tNIGHT, dust and
/// shielded coins.
pub(crate) fn dev_seed() -> Seed {
    Seed::from_hex("0000000000000000000000000000000000000000000000000000000000000001")
        .expect("dev seed")
}

/// The node URL and the indexer URL of the devnet. Returns `None`, after a
/// skip notice on stderr, when one of them is not set.
pub(crate) fn devnet_urls() -> Option<(String, String)> {
    let (Ok(node), Ok(indexer)) = (
        std::env::var("MIDNIGHT_NODE_URL"),
        std::env::var("MIDNIGHT_INDEXER_URL"),
    ) else {
        eprintln!("skipping: needs MIDNIGHT_NODE_URL + MIDNIGHT_INDEXER_URL");
        return None;
    };
    Some((node, indexer))
}

/// A provider for the devnet, with the wallet of `seed` synced and attached.
pub(crate) async fn synced_provider(node: &str, indexer: &str, seed: Seed) -> MidnightProvider {
    let provider = MidnightProvider::new(node, indexer);
    let wallet = WalletBuilder::new(seed)
        .with_network(Network::Undeployed)
        .sync(&provider)
        .await
        .expect("sync");
    provider.with_wallet(wallet)
}
