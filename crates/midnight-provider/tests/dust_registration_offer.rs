//! Dust registration builds a transaction the ledger will accept.
//!
//! Two properties are covered. First, a registration must leave out every
//! tNIGHT UTXO that already generates dust, because the ledger grants those no
//! generationless availability. Second, the tNIGHT UTXOs it does spend all
//! belong in the guaranteed offer, because that is the only leg
//! `generationless_fee_availability` sums and the only one
//! `apply_registration` creates the initial dust outputs from.
//!
//! Both tests build the registration and never submit it, so they leave the
//! chain untouched.
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

async fn dev_provider(recorder: Arc<ShapeRecorder>) -> Option<MidnightProvider> {
    let (Ok(node_url), Ok(indexer_url)) = (
        std::env::var("MIDNIGHT_NODE_URL"),
        std::env::var("MIDNIGHT_INDEXER_URL"),
    ) else {
        eprintln!("skipping: needs MIDNIGHT_NODE_URL + MIDNIGHT_INDEXER_URL");
        return None;
    };
    let seed = WalletSeed::try_from_hex_str(DEV_WALLET_SEED).unwrap();
    Some(
        MidnightProvider::new(&node_url, &indexer_url)
            .expect("provider")
            .with_proof_provider(recorder)
            .sync_wallet(seed, Network::Undeployed)
            .await
            .expect("sync"),
    )
}

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

/// The sync carries both fields a registration depends on. Dropping either one
/// from the GraphQL selection set leaves it `None`, which silently returns
/// `register_dust` to declaring a fee allowance the node rejects.
#[tokio::test]
async fn the_sync_carries_creation_time_and_registration_status() {
    let Some(provider) = dev_provider(Arc::new(ShapeRecorder::default())).await else {
        return;
    };

    let utxos = provider.unshielded_utxos().await.expect("wallet");
    let night: Vec<_> = utxos.iter().filter(|u| u.is_night()).collect();
    assert!(!night.is_empty(), "the wallet needs tNIGHT for this test");

    for utxo in night {
        assert!(
            utxo.ctime.is_some(),
            "the indexer must report a creation time, got {utxo:?}"
        );
        assert!(
            utxo.registered_for_dust_generation.is_some(),
            "the indexer must report the dust registration status, got {utxo:?}"
        );
    }
}

/// A wallet whose tNIGHT already generates dust must not build a second
/// registration. The ledger would report zero availability against a positive
/// `allow_fee_payment` and the node would reject the transaction, so the
/// refusal has to come before proving.
#[tokio::test]
async fn a_second_registration_is_refused_before_proving() {
    let recorder = Arc::new(ShapeRecorder::default());
    let Some(provider) = dev_provider(recorder.clone()).await else {
        return;
    };

    let (total, unregistered) = night_counts(&provider).await;
    assert!(total > 0, "the wallet needs tNIGHT for this test");
    if unregistered > 0 {
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
        recorder.shape().is_none(),
        "the refusal must land before the prover runs"
    );
}

/// A registration spends exactly one tNIGHT UTXO, and it sits in the
/// guaranteed offer.
///
/// One, because a second unshielded input costs more verification than its
/// bytes buy in dismissal allowance and the ledger refuses the transaction
/// with `FeeCalculation(OutsideTimeToDismiss)` (code 168). Guaranteed, because
/// the ledger sums the availability backing `allow_fee_payment` over that leg
/// alone, so an input in the fallible leg is declared and never counted
/// (`InsufficientDustForRegistrationFee`, code 173).
#[tokio::test]
async fn a_registration_spends_one_night_input_in_the_guaranteed_leg() {
    let recorder = Arc::new(ShapeRecorder::default());
    let Some(provider) = dev_provider(recorder.clone()).await else {
        return;
    };

    // The dev preset registers its genesis wallets, so this runs only against
    // a network whose funded wallet still awaits a registration.
    let (_, unregistered) = night_counts(&provider).await;
    if unregistered == 0 {
        eprintln!("skipping: every tNIGHT UTXO of this wallet already generates dust");
        return;
    }

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
        shape.guaranteed_inputs, 1,
        "a registration spends one tNIGHT input; a second one puts the \
         transaction outside its time to dismiss"
    );
}
