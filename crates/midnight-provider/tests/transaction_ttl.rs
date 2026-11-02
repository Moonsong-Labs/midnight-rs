//! A transaction built with a TTL shorter than the chain's `global_ttl` lands.
//!
//! The build counts the TTL from the chain time of the wallet's last sync,
//! not from the Dust anchor that the wallet proves against. Each intent of the
//! transaction carries that TTL, and the chain refuses one that has passed at
//! the block that applies it.
//!
//! Gated on a running devnet (`MIDNIGHT_NODE_URL`, `MIDNIGHT_INDEXER_URL`).

#[expect(dead_code, reason = "this test needs no synced_provider")]
mod common;

use std::time::Duration;

use midnight_provider::{LedgerVersion, MidnightProvider, NIGHT};

/// Long enough for a build, its proof and its inclusion, and well short of
/// the devnet's `global_ttl`.
const TTL: Duration = Duration::from_secs(15 * 60);

/// The TTL of each intent of the proven transaction `bytes`, in seconds.
fn intent_ttls(bytes: &[u8], ledger: LedgerVersion) -> Vec<u64> {
    macro_rules! ttls {
        ($ledger:ident) => {{
            let tx: midnight_helpers::$ledger::FinalizedTransaction<midnight_helpers::DefaultDB> =
                midnight_helpers::$ledger::midnight_serialize::tagged_deserialize(&mut &bytes[..])
                    .expect("a proven transaction");
            tx.intents()
                .map(|(_, intent)| intent.ttl.to_secs())
                .collect()
        }};
    }
    match ledger {
        LedgerVersion::V8 => ttls!(ledger_8),
        LedgerVersion::V9 => ttls!(ledger_9),
    }
}

/// The time of the node's best block, in seconds.
async fn chain_now(provider: &MidnightProvider) -> u64 {
    let best = provider.best_block_hash().await.expect("best block");
    provider
        .get_block_timestamp(best)
        .await
        .expect("best block time")
        .as_secs()
}

/// Each TTL is [`TTL`] after a chain time the build read: past the chain's
/// time now, and no later than [`TTL`] after it.
fn assert_short(ttls: &[u64], now: u64, what: &str) {
    assert!(!ttls.is_empty(), "{what} carries no intent");
    for &ttl in ttls {
        assert!(
            ttl > now && ttl <= now + TTL.as_secs(),
            "{what}: an intent TTL of {ttl} is outside ({now}, {}]",
            now + TTL.as_secs()
        );
    }
}

#[tokio::test]
async fn a_transfer_with_a_short_ttl_lands() {
    let Some((node_url, indexer_url)) = common::devnet_urls() else {
        return;
    };

    let provider = MidnightProvider::new(&node_url);
    let wallet = common::synced_wallet(&provider, &indexer_url, common::DEV_WALLET_SEED).await;
    let global_ttl = wallet.parameters().global_ttl.as_seconds();
    assert!(
        global_ttl > i128::from(TTL.as_secs()),
        "the devnet's global_ttl of {global_ttl}s leaves no room for a shorter TTL"
    );
    let recipient = wallet.unshielded_address();
    let provider = provider.with_wallet(wallet);

    // A transfer that pays its own fee: its intent and its fee intent are one.
    let paid = provider
        .transfer_unshielded(NIGHT, 1, &recipient)
        .with_ttl(TTL)
        .build()
        .await
        .expect("build a transfer with a short TTL");
    assert_short(
        &intent_ttls(&paid.tx_bytes, paid.ledger_version),
        chain_now(&provider).await,
        "the self-funded transfer",
    );
    provider
        .submit_reserved(paid)
        .await
        .expect("submit the self-funded transfer")
        .wait_finalized()
        .await
        .expect("the chain applies a transaction with a short TTL");
}
