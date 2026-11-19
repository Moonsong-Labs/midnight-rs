//! A contract call whose circuit emits an event applies on chain, and the
//! simulation and the call return the event.
//!
//! The local run puts the `push` and the `log` of the event in the transcript
//! that the SDK proves and submits. The chain replays that transcript and
//! checks it against the proof. So the chain accepts the call only when the
//! local run builds the event exactly as the circuit does, and the event that
//! the call returns is the one that the chain emits. `Contract::latest_events`
//! and `Contract::indexer_events_at` give the same event from the indexer, as
//! the event of the call's transaction.
//!
//! Gated on a running devnet (`MIDNIGHT_NODE_URL`, `MIDNIGHT_INDEXER_URL`).
//! Under `make test-e2e`, which sets `MIDNIGHT_E2E`, a missing URL panics.

mod common;

mod events {
    compact_bindgen::contract!("../../devnet/contracts/events/compiled/compiler/analyzed-ir.sexp");
}

use compact_bindgen::Bytes;
use midnight_contract::runtime::Misc;
use midnight_contract::{ContractError, ContractEvent, FromBlock};
use midnight_provider::MidnightProvider;

const NAME: [u8; 32] = [0x07; 32];
/// The text that `record` pads to the 256 bytes of the event's `payload`.
const PAYLOAD_TEXT: &[u8] = b"midnight-rs";

#[tokio::test]
async fn a_call_that_emits_an_event_applies_on_chain_and_returns_it() {
    let Some((node_url, indexer_url)) = common::devnet_urls() else {
        return;
    };
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
        .send()
        .await
        .expect("submit deploy")
        .into_contract()
        .await
        .unwrap_or_else(|e| panic!("the deploy must apply: {e:?}"));

    let simulated = contract
        .circuits()
        .record(Bytes(NAME))
        .simulate()
        .await
        .unwrap_or_else(|e| panic!("the simulation must run: {e:?}"));
    let [event] = simulated.events.as_slice() else {
        panic!("record emits one event, got {:?}", simulated.events);
    };
    let misc = Misc::decode_event(event).expect("record emits a Misc event");
    let mut payload = [0u8; 256];
    payload[..PAYLOAD_TEXT.len()].copy_from_slice(PAYLOAD_TEXT);
    assert_eq!(
        (misc.name, misc.payload),
        (Bytes(NAME), Bytes(payload)),
        "the Misc event holds the name and the padded text"
    );

    // The call lands in a block after the indexer's latest block now.
    let start = contract
        .latest_events(&indexer_url, FromBlock::Latest)
        .await
        .unwrap_or_else(|e| panic!("the indexer read must succeed: {e:?}"))
        .next_height;

    // `get_receipt` waits for finality, so the test ends on a final
    // block. The next test in the suite is another process on the same seed,
    // and its wallet reads finalized state only.
    let outcome = contract
        .circuits()
        .record(Bytes(NAME))
        .send()
        .await
        .expect("submit record")
        .get_receipt()
        .await
        .unwrap_or_else(|e| panic!("the emitting call must apply: {e:?}"));
    assert_eq!(
        outcome.events, simulated.events,
        "the call must return the event that its run emitted"
    );

    let expected = vec![ContractEvent {
        transaction_hash: outcome.transaction_hash,
        entry_point: b"record".to_vec(),
        item: event.clone(),
    }];

    // The indexer trails finality, so read from `start` until it holds the
    // call's block. The test deployed the contract, so no other block holds
    // an event of it.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let mut from = FromBlock::Height(start);
    let block = loop {
        match contract.latest_events(&indexer_url, from).await {
            Ok(read) => {
                if let [block] = read.blocks.as_slice() {
                    break block.clone();
                }
                assert!(
                    read.blocks.is_empty(),
                    "one block holds an event, got {:?}",
                    read.blocks
                );
                from = FromBlock::Height(read.next_height);
            }
            Err(ContractError::Indexer {
                retryable: true, ..
            }) => {}
            Err(e) => panic!("the indexer read must succeed: {e:?}"),
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the indexer must index block {:x} within 60 s",
            outcome.block_hash
        );
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    };
    assert_eq!(
        (block.hash, block.events),
        (outcome.block_hash, expected.clone()),
        "the indexer gives the event of the call in the call's block"
    );
    let at_block = contract
        .indexer_events_at(&indexer_url, outcome.block_hash)
        .await
        .unwrap_or_else(|e| panic!("the indexer read of the call's block must succeed: {e:?}"));
    assert_eq!(
        at_block,
        Some(expected),
        "the indexer read of the call's block gives the event of the call"
    );
}
