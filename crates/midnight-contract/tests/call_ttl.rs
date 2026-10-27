//! A deploy and a circuit call built with a TTL shorter than the chain's
//! `global_ttl` land, and the fee intent of the call expires with it.
//!
//! The generated builders hand the TTL down to the build. A call proves with
//! no fee, so its TTL rides its own intent, and the wallet's fee intent,
//! which joins after the proof, takes the same TTL.
//!
//! Gated on a running devnet (`MIDNIGHT_NODE_URL`, `MIDNIGHT_INDEXER_URL`).

mod common;

mod counter {
    compact_bindgen::contract!("../../devnet/contracts/counter/compiled/compiler/analyzed-ir.sexp");
}

use std::time::Duration;

use midnight_provider::{LedgerVersion, MidnightProvider};

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

#[tokio::test]
async fn a_deploy_and_a_call_with_a_short_ttl_land() {
    let Some((node_url, indexer_url)) = common::devnet_urls() else {
        return;
    };

    let provider = MidnightProvider::new(&node_url);
    let wallet = common::synced_wallet(&provider, &indexer_url, common::DEV_WALLET_SEED).await;
    let provider = provider.with_wallet(wallet);

    let contract = counter::Contract::deploy(&provider)
        .with_initial_state(counter::LedgerInitialState::default())
        .with_zk_config(counter::ZK_CONFIG_DIR)
        .with_ttl(TTL)
        .await
        .expect("the chain applies a deploy with a short TTL");
    let round_before = contract
        .ledger()
        .await
        .expect("ledger")
        .round()
        .expect("round");

    let bytes = contract
        .circuits()
        .increment()
        .with_ttl(TTL)
        .build()
        .await
        .expect("build a call with a short TTL");
    let ledger = provider.ledger_version().await.expect("ledger version");
    let ttls = intent_ttls(&bytes, ledger);
    let best = provider.best_block_hash().await.expect("best block");
    let now = provider
        .get_block_timestamp(best)
        .await
        .expect("best block time")
        .as_secs();
    assert_eq!(ttls.len(), 2, "the call's intent and its fee intent");
    assert!(
        ttls.iter()
            .all(|&ttl| ttl > now && ttl <= now + TTL.as_secs()),
        "each intent TTL must be {TTL:?} after a chain time the build read: \
         {ttls:?} against a chain time of {now}"
    );
    assert_eq!(ttls[0], ttls[1], "the fee intent must expire with the call");

    // The next build on this seed must see this spend finalized, or it draws
    // the same Dust.
    provider
        .submit(&bytes)
        .await
        .expect("submit the call")
        .wait_finalized()
        .await
        .expect("the chain applies a call with a short TTL");
    let round_after = contract
        .ledger()
        .await
        .expect("ledger")
        .round()
        .expect("round");
    assert_eq!(round_after, round_before + 1, "the call must have run");
}
