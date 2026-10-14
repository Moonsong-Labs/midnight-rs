//! Devnet setup for the devnet tests of this crate.

use midnight_provider::{MidnightProvider, Network};
use midnight_wallet::Wallet;

/// The seed of the devnet genesis wallet, which holds tNIGHT, dust and
/// shielded coins.
pub(crate) const DEV_WALLET_SEED: &str =
    "0000000000000000000000000000000000000000000000000000000000000001";

/// Prints a skip line on stderr for the precondition `missing`. The caller
/// then returns without running the test.
///
/// # Panics
///
/// When `MIDNIGHT_E2E` is set. The make targets that run the devnet tests set
/// it, so a test that they run never skips.
pub(crate) fn skip(missing: &str) {
    if std::env::var_os("MIDNIGHT_E2E").is_some() {
        panic!("{missing} is missing while MIDNIGHT_E2E is set");
    }
    eprintln!("skipping: {missing} is missing");
}

/// The node URL and the indexer URL of the devnet. Returns `None`, after a
/// [`skip`], when one of them is not set.
pub(crate) fn devnet_urls() -> Option<(String, String)> {
    let (Ok(node), Ok(indexer)) = (
        std::env::var("MIDNIGHT_NODE_URL"),
        std::env::var("MIDNIGHT_INDEXER_URL"),
    ) else {
        skip("MIDNIGHT_NODE_URL or MIDNIGHT_INDEXER_URL");
        return None;
    };
    Some((node, indexer))
}

/// The wallet of the hex seed `seed` on the undeployed network, synced
/// through `provider`.
pub(crate) async fn synced_wallet(provider: &MidnightProvider, seed: &str) -> Wallet {
    Wallet::from_seed(seed)
        .expect("hex seed")
        .with_network(Network::Undeployed)
        .sync(provider)
        .await
        .expect("sync")
}

/// A provider for the devnet, with the wallet of `seed` synced and attached.
pub(crate) async fn synced_provider(node: &str, indexer: &str, seed: &str) -> MidnightProvider {
    let provider = MidnightProvider::new(node, indexer);
    let wallet = synced_wallet(&provider, seed).await;
    provider.with_wallet(wallet)
}
