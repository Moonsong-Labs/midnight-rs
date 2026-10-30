//! A contract call whose circuit emits an event applies on chain, and the
//! simulation and the call return the event.
//!
//! The local run puts the `push` and the `log` of the event in the transcript
//! that the SDK proves and submits. The chain replays that transcript and
//! checks it against the proof. So the chain accepts the call only when the
//! local run builds the event exactly as the circuit does, and the event that
//! the call returns is the one that the chain logs.
//!
//! Gated on a running devnet (`MIDNIGHT_NODE_URL`, `MIDNIGHT_INDEXER_URL`).
//! Under `make test-e2e`, which sets `MIDNIGHT_E2E`, a missing URL or a
//! missing `MIDNIGHT_LEDGER` panics.

mod common;

mod events {
    compact_bindgen::contract!("../../devnet/contracts/events/compiled/compiler/analyzed-ir.sexp");
}

use compact_bindgen::{Bytes, cell_as};
use midnight_contract::runtime::LogEventType;
use midnight_provider::MidnightProvider;

const NAME: [u8; 32] = [0x07; 32];
/// The text that `record` pads to the 256 bytes of the event's `payload`.
const PAYLOAD_TEXT: &[u8] = b"midnight-rs";

#[tokio::test]
async fn a_call_that_emits_an_event_applies_on_chain_and_returns_it() {
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

    let provider = MidnightProvider::new(&node_url);
    let wallet = common::synced_wallet(&provider, &indexer_url, common::DEV_WALLET_SEED).await;
    let provider = provider.with_wallet(wallet);

    let contract = events::Contract::deploy(&provider)
        .with_initial_state(events::LedgerInitialState::default())
        .with_zk_config(events::ZK_CONFIG_DIR)
        .await
        .unwrap_or_else(|e| panic!("the deploy must apply on ledger {ledger}: {e:?}"));

    let simulated = contract
        .circuits()
        .record(Bytes(NAME))
        .simulate()
        .await
        .unwrap_or_else(|e| panic!("the simulation must run on ledger {ledger}: {e:?}"));
    let [event] = simulated.logs.as_slice() else {
        panic!("record emits one event, got {:?}", simulated.logs);
    };
    assert_eq!((event.version, event.event_type), (1, LogEventType::Misc));
    let payload: Bytes<288> = cell_as(&event.data).expect("the Misc payload is one cell");
    let mut expected = [0u8; 288];
    expected[..NAME.len()].copy_from_slice(&NAME);
    expected[NAME.len()..NAME.len() + PAYLOAD_TEXT.len()].copy_from_slice(PAYLOAD_TEXT);
    assert_eq!(
        payload.0, expected,
        "the Misc payload is the name, then the padded text"
    );

    // The call's `.await` waits for finality, so the test ends on a final
    // block. The next test in the suite is another process on the same seed,
    // and its wallet reads finalized state only.
    let outcome = contract
        .circuits()
        .record(Bytes(NAME))
        .await
        .unwrap_or_else(|e| panic!("the emitting call must apply on ledger {ledger}: {e:?}"));
    assert_eq!(
        outcome.logs, simulated.logs,
        "the call must return the event that its run emitted"
    );
}
