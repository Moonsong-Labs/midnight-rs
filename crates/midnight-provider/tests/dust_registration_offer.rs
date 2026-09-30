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

use midnight_wallet::{LocalWallet, Wallet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use midnight_helpers::{
    CostModel, DefaultDB, LocalProofServer, PedersenRandomness, ProofMarker, ProofPreimageMarker,
    ProofProvider, Resolver, Signature, StdRng, Transaction,
};
use midnight_provider::{MidnightProvider, Network, WalletSeed};

const DEV_WALLET_SEED: &str = "0000000000000000000000000000000000000000000000000000000000000001";

/// Wraps the real prover and records whether it ran.
#[derive(Default)]
struct ProofRecorder {
    inner: LocalProofServer,
    proved: AtomicBool,
}

#[async_trait::async_trait]
impl ProofProvider<DefaultDB> for ProofRecorder {
    async fn prove(
        &self,
        tx: Transaction<Signature, ProofPreimageMarker, PedersenRandomness, DefaultDB>,
        rng: StdRng,
        resolver: &Resolver,
        cost_model: &CostModel,
    ) -> Transaction<Signature, ProofMarker, PedersenRandomness, DefaultDB> {
        self.proved.store(true, Ordering::SeqCst);
        self.inner.prove(tx, rng, resolver, cost_model).await
    }
}

async fn dev_provider(recorder: Arc<ProofRecorder>) -> Option<MidnightProvider> {
    let (Ok(node_url), Ok(indexer_url)) = (
        std::env::var("MIDNIGHT_NODE_URL"),
        std::env::var("MIDNIGHT_INDEXER_URL"),
    ) else {
        eprintln!("skipping: needs MIDNIGHT_NODE_URL + MIDNIGHT_INDEXER_URL");
        return None;
    };
    let seed = WalletSeed::try_from_hex_str(DEV_WALLET_SEED).unwrap();
    let provider = MidnightProvider::new(&node_url, &indexer_url)
        .expect("provider")
        .with_proof_provider(recorder);
    let wallet = Wallet::sync(provider.indexer_url(), seed, Network::Undeployed)
        .await
        .expect("sync");
    Some(provider.with_wallet(LocalWallet::new(wallet)))
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

/// A wallet whose tNIGHT already generates dust must not build a second
/// registration. The ledger would report zero availability against a positive
/// `allow_fee_payment` and the node would reject the transaction, so the
/// refusal has to come before proving.
#[tokio::test]
async fn a_second_registration_is_refused_before_proving() {
    let recorder = Arc::new(ProofRecorder::default());
    let Some(provider) = dev_provider(recorder.clone()).await else {
        return;
    };

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
