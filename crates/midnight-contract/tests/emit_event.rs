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

mod events {
    compact_bindgen::contract!("../../devnet/contracts/events/compiled/compiler/analyzed-ir.sexp");
}

use compact_bindgen::Bytes;
use midnight_provider::{MidnightProvider, Network, WalletSeed};
use midnight_wallet::{LocalWallet, Wallet};

const ZK_KEYS_DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../devnet/contracts/events/compiled"
);
const DEV_WALLET_SEED: &str = "0000000000000000000000000000000000000000000000000000000000000001";
const NAME: [u8; 32] = [0x07; 32];

#[tokio::test]
async fn a_call_that_emits_an_event_applies_on_chain() {
    let (Ok(node_url), Ok(indexer_url)) = (
        std::env::var("MIDNIGHT_NODE_URL"),
        std::env::var("MIDNIGHT_INDEXER_URL"),
    ) else {
        if std::env::var_os("MIDNIGHT_E2E").is_some() {
            panic!("MIDNIGHT_NODE_URL or MIDNIGHT_INDEXER_URL is missing under make test-e2e");
        }
        eprintln!("skipping: needs MIDNIGHT_NODE_URL + MIDNIGHT_INDEXER_URL");
        return;
    };
    let ledger = std::env::var("MIDNIGHT_LEDGER").unwrap_or_else(|_| {
        if std::env::var_os("MIDNIGHT_E2E").is_some() {
            panic!("MIDNIGHT_LEDGER is missing under make test-e2e");
        }
        "unset".into()
    });
    assert!(
        std::path::Path::new(ZK_KEYS_DIR).join("keys").is_dir(),
        "{ZK_KEYS_DIR}/keys is missing; run `make compile-contracts CONTRACTS=events`"
    );

    let seed = WalletSeed::try_from_hex_str(DEV_WALLET_SEED).unwrap();
    let provider = MidnightProvider::new(&node_url, &indexer_url).expect("provider");
    let wallet = Wallet::sync(&provider, seed, Network::Undeployed)
        .await
        .expect("sync");
    let provider = provider.with_wallet(LocalWallet::new(wallet));

    let contract = events::Contract::deploy(&provider)
        .with_initial_state(events::LedgerInitialState::default())
        .with_zk_config(ZK_KEYS_DIR)
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
