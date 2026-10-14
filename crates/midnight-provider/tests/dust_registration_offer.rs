//! Dust registration refuses a wallet that has nothing to register.
//!
//! A registration must leave out every tNIGHT UTXO that already generates
//! dust, because the ledger grants those no generationless availability. A
//! wallet whose tNIGHT all generates dust has nothing to register, so the
//! build must refuse it before proving.
//!
//! The test never submits, so it leaves the chain untouched.
//!
//! Gated on a running devnet (`MIDNIGHT_NODE_URL`, `MIDNIGHT_INDEXER_URL`).

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use midnight_helpers::{DefaultDB, StdRng};
use midnight_provider::MidnightProvider;

/// Wraps each generation's real prover and records whether one ran.
#[derive(Default)]
struct ProofRecorder {
    ledger_8: midnight_helpers::ledger_8::LocalProofServer,
    ledger_9: midnight_helpers::ledger_9::LocalProofServer,
    proved: AtomicBool,
}

/// The recorder for each generation; the body is the same on each.
macro_rules! proof_recorder {
    ($ledger:ident) => {
        #[async_trait::async_trait]
        impl midnight_helpers::$ledger::ProofProvider<DefaultDB> for ProofRecorder {
            async fn prove(
                &self,
                tx: midnight_helpers::$ledger::UnprovenTransaction<DefaultDB>,
                rng: StdRng,
                resolver: &'static midnight_helpers::$ledger::Resolver,
                cost_model: midnight_helpers::$ledger::CostModel,
            ) -> midnight_helpers::$ledger::ProvenTransaction<DefaultDB> {
                self.proved.store(true, Ordering::SeqCst);
                self.$ledger.prove(tx, rng, resolver, cost_model).await
            }
        }
    };
}

proof_recorder!(ledger_8);
proof_recorder!(ledger_9);

/// Counts the wallet's tNIGHT UTXOs, and how many of them still await a
/// registration.
async fn night_counts(provider: &MidnightProvider) -> (usize, usize) {
    let utxos = provider.unshielded_utxos().await.expect("wallet");
    let night = utxos.iter().filter(|u| u.is_night());
    let total = night.clone().count();
    let unregistered = night
        .filter(|u| u.registered_for_dust_generation != Some(true))
        .count();
    (total, unregistered)
}

/// A wallet whose tNIGHT already generates dust must not build a second
/// registration. The ledger would report zero availability against a positive
/// `allow_fee_payment` and the node would reject the transaction, so the
/// refusal has to come before proving.
#[tokio::test]
async fn a_second_registration_is_refused_before_proving() {
    let Some((node, indexer)) = common::devnet_urls() else {
        return;
    };
    let recorder = Arc::new(ProofRecorder::default());
    let provider = common::synced_provider(&node, &indexer, common::dev_seed())
        .await
        .with_proof_provider(recorder.clone());

    let (total, unregistered) = night_counts(&provider).await;
    assert!(total > 0, "the wallet needs tNIGHT for this test");
    if unregistered > 0 {
        if std::env::var_os("MIDNIGHT_E2E").is_some() {
            panic!(
                "a wallet whose every tNIGHT UTXO generates dust is missing under make test-e2e: \
                 {unregistered} are unregistered"
            );
        }
        eprintln!("skipping: this wallet still has {unregistered} unregistered tNIGHT UTXOs");
        return;
    }

    let Err(err) = provider.register_dust(None).build().await else {
        panic!("a registered wallet must not build another registration");
    };
    assert!(
        err.to_string().contains("already generates dust"),
        "the error must name the cause, got: {err}"
    );
    assert!(
        !recorder.proved.load(Ordering::SeqCst),
        "the refusal must land before the prover runs"
    );
}
