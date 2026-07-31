//! A dust registration must keep every tNIGHT input in the guaranteed offer.
//!
//! The ledger sums the availability backing `allow_fee_payment` over the
//! parent intent's guaranteed inputs only (`generationless_fee_availability`),
//! and `apply_registration` creates the initial dust outputs from that same
//! offer. Splitting the inputs across the guaranteed and fallible legs makes
//! the builder declare more than the ledger counts, and the node rejects the
//! transaction with `InsufficientDustForRegistrationFee` (custom error 173).
//!
//! The test inspects the transaction handed to the prover, so it builds but
//! never submits.
//!
//! Gated on a running devnet (`MIDNIGHT_NODE_URL`, `MIDNIGHT_INDEXER_URL`).

use std::sync::Arc;
use std::sync::Mutex;

use midnight_helpers::{
    CostModel, DefaultDB, LocalProofServer, PedersenRandomness, ProofMarker, ProofPreimageMarker,
    ProofProvider, Resolver, Signature, StdRng, Transaction,
};
use midnight_provider::{MidnightProvider, Network, WalletSeed};

const DEV_WALLET_SEED: &str = "0000000000000000000000000000000000000000000000000000000000000001";

/// The shape of the one intent that carries the dust registration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RegistrationShape {
    guaranteed_inputs: usize,
    fallible_inputs: usize,
}

/// Wraps the real prover and records the offer shape of the registration
/// intent, which is only observable before the transaction is serialized.
#[derive(Default)]
struct ShapeRecorder {
    inner: LocalProofServer,
    shape: Mutex<Option<RegistrationShape>>,
}

impl ShapeRecorder {
    fn shape(&self) -> Option<RegistrationShape> {
        *self.shape.lock().unwrap()
    }
}

#[async_trait::async_trait]
impl ProofProvider<DefaultDB> for ShapeRecorder {
    async fn prove(
        &self,
        tx: Transaction<Signature, ProofPreimageMarker, PedersenRandomness, DefaultDB>,
        rng: StdRng,
        resolver: &Resolver,
        cost_model: &CostModel,
    ) -> Transaction<Signature, ProofMarker, PedersenRandomness, DefaultDB> {
        if let Transaction::Standard(stx) = &tx {
            for kv in stx.intents.iter() {
                let intent = &kv.1;
                let registers = intent
                    .dust_actions
                    .as_ref()
                    .is_some_and(|da| da.registrations.iter().next().is_some());
                if registers {
                    *self.shape.lock().unwrap() = Some(RegistrationShape {
                        guaranteed_inputs: intent
                            .guaranteed_unshielded_offer
                            .as_ref()
                            .map_or(0, |o| o.inputs.iter().count()),
                        fallible_inputs: intent
                            .fallible_unshielded_offer
                            .as_ref()
                            .map_or(0, |o| o.inputs.iter().count()),
                    });
                }
            }
        }
        self.inner.prove(tx, rng, resolver, cost_model).await
    }
}

#[tokio::test]
async fn a_registration_keeps_every_night_input_guaranteed() {
    let (Ok(node_url), Ok(indexer_url)) = (
        std::env::var("MIDNIGHT_NODE_URL"),
        std::env::var("MIDNIGHT_INDEXER_URL"),
    ) else {
        eprintln!("skipping: needs MIDNIGHT_NODE_URL + MIDNIGHT_INDEXER_URL");
        return;
    };

    let recorder = Arc::new(ShapeRecorder::default());
    let seed = WalletSeed::try_from_hex_str(DEV_WALLET_SEED).unwrap();
    let provider = MidnightProvider::new(&node_url, &indexer_url)
        .expect("provider")
        .with_proof_provider(recorder.clone())
        .sync_wallet(seed, Network::Undeployed)
        .await
        .expect("sync");

    let night_utxos = provider
        .balance()
        .await
        .expect("balance")
        .unshielded
        .iter()
        .filter(|u| u.token_type_hex() == "0".repeat(64))
        .count();
    assert!(
        night_utxos > 0,
        "the wallet needs tNIGHT to build a registration"
    );

    // `.build()` stops before submission, so this leaves the chain untouched.
    provider
        .register_dust(None)
        .build()
        .await
        .expect("build the registration");

    let shape = recorder
        .shape()
        .expect("the prover saw an intent carrying a dust registration");

    assert_eq!(
        shape.fallible_inputs, 0,
        "a tNIGHT input in the fallible leg is declared in allow_fee_payment \
         but not counted by the ledger, so the node rejects the registration"
    );
    assert_eq!(
        shape.guaranteed_inputs, night_utxos,
        "every tNIGHT UTXO must back the declared allow_fee_payment"
    );
}
