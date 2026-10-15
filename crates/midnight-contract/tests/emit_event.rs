//! A contract call whose circuit emits an event applies on chain.
//!
//! The local run puts the `push` and the `log` of the event in the transcript
//! that the SDK proves and submits. The chain replays that transcript and
//! checks it against the proof. So the chain accepts the call only when the
//! local run builds the event exactly as the circuit does.
//!
//! Gated on a running devnet (`MIDNIGHT_NODE_URL`, `MIDNIGHT_INDEXER_URL`).
//! Under `make test-e2e`, which sets `MIDNIGHT_E2E`, a missing URL or a
//! missing `MIDNIGHT_LEDGER` panics.

mod common;

mod events {
    compact_bindgen::contract!("../../devnet/contracts/events/compiled/compiler/analyzed-ir.sexp");
}

use compact_bindgen::Bytes;
use midnight_provider::MidnightProvider;

const NAME: [u8; 32] = [0x07; 32];

#[tokio::test]
async fn a_call_that_emits_an_event_applies_on_chain() {
    let Some((node_url, indexer_url)) = common::devnet_urls() else {
        return;
    };
    let ledger = std::env::var("MIDNIGHT_LEDGER").unwrap_or_else(|_| {
        if std::env::var_os("MIDNIGHT_E2E").is_some() {
            panic!("MIDNIGHT_LEDGER is missing under make test-e2e");
        }
        "unset".into()
    });
    assert!(
        std::path::Path::new(events::ZK_CONFIG_DIR)
            .join("keys")
            .is_dir(),
        "{}/keys is missing; run `make compile-contracts CONTRACTS=events`",
        events::ZK_CONFIG_DIR
    );

    let provider = MidnightProvider::new(&node_url, &indexer_url);
    let wallet = common::synced_wallet(&provider, common::DEV_WALLET_SEED).await;
    let provider = provider.with_wallet(wallet);

    let contract = events::Contract::deploy(&provider)
        .with_initial_state(events::LedgerInitialState::default())
        .with_zk_config(events::ZK_CONFIG_DIR)
        .await
        .unwrap_or_else(|e| panic!("the deploy must apply on ledger {ledger}: {e:?}"));

    // The call's `.await` waits for finality, so the test ends on a final
    // block. The next test in the suite is another process on the same seed,
    // and its wallet reads finalized state only.
    contract
        .circuits()
        .record(Bytes(NAME))
        .await
        .unwrap_or_else(|e| panic!("the emitting call must apply on ledger {ledger}: {e:?}"));
}
