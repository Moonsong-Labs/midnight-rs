//! A contract that a remote proof server proves must run on the chain.
//!
//! [`RemoteProofServer`] proves both transactions. The deploy pays its fee with
//! a Dust spend, which the proof server proves with a built-in key. The call
//! also proves the counter's circuit, so the client sends that circuit's IR
//! and keys. On ledger 9 the counter is the ZKIR v3 build, which the proof
//! server routes to its ZKIR v3 prover. Ledger 8 cannot verify a ZKIR v3 key,
//! so there the counter is the ZKIR v2 build.
//!
//! Gated on a running devnet (`MIDNIGHT_NODE_URL`, `MIDNIGHT_INDEXER_URL`) and
//! its proof server (`MIDNIGHT_PROOF_SERVER_URL`). Under `make test-e2e`, which
//! sets `MIDNIGHT_E2E`, a missing URL panics.

mod common;

mod counter {
    compact_bindgen::contract!("../../devnet/contracts/counter/compiled/compiler/analyzed-ir.sexp");
}

mod counter_zkir_v3 {
    compact_bindgen::contract!(
        "../../devnet/contracts/counter-zkir-v3/compiled/compiler/analyzed-ir.sexp"
    );
}

use std::sync::Arc;

use midnight_provider::{LedgerVersion, MidnightProvider, RemoteProofServer};

/// Deploy the counter of the module `$counter` through `$provider`, call its
/// `increment`, and assert that the round advanced.
macro_rules! deploy_and_increment {
    ($counter:ident, $provider:expr) => {{
        let contract = $counter::Contract::deploy($provider)
            .with_initial_state($counter::LedgerInitialState::default())
            .with_zk_config($counter::ZK_CONFIG_DIR)
            .await
            .expect("the node must accept the remotely proven deploy");
        let round_before = contract
            .ledger()
            .await
            .expect("ledger")
            .round()
            .expect("round");

        contract
            .circuits()
            .increment()
            .await
            .expect("the node must accept the remotely proven call");

        let round_after = contract
            .ledger()
            .await
            .expect("ledger")
            .round()
            .expect("round");
        assert_eq!(
            round_after,
            round_before + 1,
            "the call should have advanced the counter"
        );
    }};
}

#[tokio::test]
async fn a_remotely_proven_call_advances_the_counter() {
    let Some((node_url, indexer_url)) = common::devnet_urls() else {
        return;
    };
    let Some(proof_server_url) = common::proof_server_url() else {
        return;
    };

    let provider = MidnightProvider::new(&node_url)
        .with_proof_provider(Arc::new(RemoteProofServer::new(proof_server_url)));
    let ledger = provider.ledger_version().await.expect("ledger version");
    let wallet = common::synced_wallet(&provider, &indexer_url, common::DEV_WALLET_SEED).await;
    let provider = provider.with_wallet(wallet);

    match ledger {
        LedgerVersion::V8 => deploy_and_increment!(counter, &provider),
        LedgerVersion::V9 => deploy_and_increment!(counter_zkir_v3, &provider),
    }
}
