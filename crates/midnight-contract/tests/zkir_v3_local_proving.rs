//! A contract compiled for ZKIR v3 must run on a ledger 9 chain with the
//! default prover, and a ledger 8 deploy must refuse its keys.
//!
//! The default prover proves in this process. On ledger 9 it must route the
//! `ir-source[v3-generic]` IR of the call's circuit to the ZKIR v3 prover, and
//! keep the ZKIR v2 prover for the Dust spend of the fee. A ledger 8 chain
//! cannot verify a `verifier-key[v7]`, so the deploy must fail before it
//! submits, typed as `ContractError::Construction`.
//!
//! Gated on a running devnet (`MIDNIGHT_NODE_URL`, `MIDNIGHT_INDEXER_URL`).
//! Under `make test-e2e`, which sets `MIDNIGHT_E2E`, a missing URL panics.

mod common;

mod counter {
    compact_bindgen::contract!(
        "../../devnet/contracts/counter-zkir-v3/compiled/compiler/analyzed-ir.sexp"
    );
}

use midnight_contract::ContractError;
use midnight_provider::{LedgerVersion, MidnightProvider};

#[tokio::test]
async fn a_zkir_v3_contract_runs_only_on_ledger_9() {
    let Some((node_url, indexer_url)) = common::devnet_urls() else {
        return;
    };

    let provider = MidnightProvider::new(&node_url, &indexer_url);
    let ledger = provider.ledger_version().await.expect("ledger version");
    let wallet = common::synced_wallet(&provider, common::DEV_WALLET_SEED).await;
    let provider = provider.with_wallet(wallet);

    let deploy = counter::Contract::deploy(&provider)
        .with_initial_state(counter::LedgerInitialState::default())
        .with_zk_config(counter::ZK_CONFIG_DIR)
        .await;

    match ledger {
        LedgerVersion::V8 => match deploy {
            Err(ContractError::Construction(msg)) => assert!(
                msg.contains("verifier-key[v7]"),
                "expected the refusal of a ZKIR v3 key, got {msg}"
            ),
            Err(other) => panic!("expected a construction error, got {other:?}"),
            Ok(_) => panic!("a ledger 8 chain must not take a ZKIR v3 contract"),
        },
        LedgerVersion::V9 => {
            let contract = deploy.expect("deploy");
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
                .expect("the node must accept the call");

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
        }
    }
}
