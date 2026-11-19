//! A contract call whose circuit emits an event applies on chain, and the
//! simulation and the call return the event.
//!
//! The local run puts the `push` and the `log` of the event in the transcript
//! that the SDK proves and submits. The chain replays that transcript and
//! checks it against the proof. So the chain accepts the call only when the
//! local run builds the event exactly as the circuit does, and the event that
//! the call returns is the one that the chain emits. A read of the call's block
//! from the node gives the same event, from the transcript that the chain
//! applied. `Contract::events_at` gives it as the event of the call's
//! transaction. `Contract::latest_events` and `Contract::indexer_events_at`
//! give the same event from the indexer.
//!
//! Gated on a running devnet (`MIDNIGHT_NODE_URL`, `MIDNIGHT_INDEXER_URL`).
//! Under `make test-e2e`, which sets `MIDNIGHT_E2E`, a missing URL panics.

mod common;

mod events {
    compact_bindgen::contract!("../../devnet/contracts/events/compiled/compiler/analyzed-ir.sexp");
}

use compact_bindgen::Bytes;
use midnight_contract::runtime::Misc;
use midnight_contract::{
    CallStatus, ContractError, ContractEvent, FromBlock, call_transcripts, parse_contract_address,
};
use midnight_provider::{MidnightProvider, NodeApi, TxOutcome};

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

    let ran = provider
        .get_block_transactions(outcome.block_hash)
        .await
        .unwrap_or_else(|e| panic!("the read of the call's block must succeed: {e:?}"));
    let transaction = ran
        .iter()
        .find(|transaction| transaction.transaction_hash == outcome.transaction_hash)
        .unwrap_or_else(|| {
            let hashes: Vec<_> = ran
                .iter()
                .map(|transaction| transaction.transaction_hash)
                .collect();
            panic!("the block must run the call's transaction, got {hashes:?}")
        });
    assert_eq!(transaction.outcome, TxOutcome::Applied);
    let bytes = transaction
        .bytes
        .as_deref()
        .expect("the block body holds the call's transaction");
    let calls = call_transcripts(bytes, transaction.outcome).expect("the transaction decodes");
    let [call] = calls.as_slice() else {
        panic!("the call has one transcript, got {}", calls.len());
    };
    assert_eq!(
        (call.address, call.entry_point.as_slice(), call.status),
        (
            parse_contract_address(contract.address()).expect("a contract address"),
            b"record".as_slice(),
            CallStatus::Applied,
        )
    );
    assert_eq!(
        call.events()
            .expect("the events of compactc output read statically"),
        outcome.events,
        "the transcript that the chain applied must emit the events of the run"
    );

    let node_events = contract
        .events_at(outcome.block_hash)
        .await
        .unwrap_or_else(|e| panic!("the node read of the call's block must succeed: {e:?}"));
    assert_eq!(
        node_events,
        [ContractEvent {
            transaction_hash: outcome.transaction_hash,
            entry_point: b"record".to_vec(),
            item: event.clone(),
        }],
        "the node read gives the event of the call"
    );

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
        (outcome.block_hash, node_events.clone()),
        "the indexer gives the events of the call's block that the node gives"
    );
    let at_block = contract
        .indexer_events_at(&indexer_url, outcome.block_hash)
        .await
        .unwrap_or_else(|e| panic!("the indexer read of the call's block must succeed: {e:?}"));
    assert_eq!(
        at_block,
        Some(node_events),
        "the indexer read of the call's block gives the events that the node gives"
    );
}
