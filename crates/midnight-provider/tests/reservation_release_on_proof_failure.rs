//! A build reserves its inputs before it proves, so a failed proof has to hand
//! them back.
//!
//! Selection and the reservation happen while the wallet is held, and proving
//! runs after it is released. That ordering is what keeps proving out of the
//! wallet's critical section, and it means a reservation now outlives the
//! decision that made it. If proving fails and nothing releases it, the inputs
//! sit reserved until their TTL elapses and the wallet looks poorer than it is.
//!
//! The test proves nothing about the happy path; the e2e suite covers that.
//!
//! Gated on a running devnet (`MIDNIGHT_NODE_URL`, `MIDNIGHT_INDEXER_URL`).

use std::sync::Arc;

use midnight_helpers::{
    CostModel, DefaultDB, PedersenRandomness, ProofMarker, ProofPreimageMarker, ProofProvider,
    Resolver, Signature, StdRng, Transaction,
};
use midnight_provider::{MidnightProvider, Network, WalletSeed};

const DEV_WALLET_SEED: &str = "0000000000000000000000000000000000000000000000000000000000000001";

/// A prover that always fails. `ProofProvider::prove` returns a bare
/// transaction, so failing means unwinding; the build path catches that and
/// reports `WalletError::Proving`.
struct BrokenProver;

#[async_trait::async_trait]
impl ProofProvider<DefaultDB> for BrokenProver {
    async fn prove(
        &self,
        _tx: Transaction<Signature, ProofPreimageMarker, PedersenRandomness, DefaultDB>,
        _rng: StdRng,
        _resolver: &Resolver,
        _cost_model: &CostModel,
    ) -> Transaction<Signature, ProofMarker, PedersenRandomness, DefaultDB> {
        panic!("prover is down");
    }
}

#[tokio::test]
async fn a_failed_proof_hands_the_reserved_coins_back() {
    let (Ok(node), Ok(indexer)) = (
        std::env::var("MIDNIGHT_NODE_URL"),
        std::env::var("MIDNIGHT_INDEXER_URL"),
    ) else {
        eprintln!("skipping: needs MIDNIGHT_NODE_URL + MIDNIGHT_INDEXER_URL");
        return;
    };

    let seed = WalletSeed::try_from_hex_str(DEV_WALLET_SEED).expect("dev seed");
    let provider = MidnightProvider::new(&node, &indexer)
        .expect("provider")
        .sync_wallet(seed.clone(), Network::Undeployed)
        .await
        .expect("sync");

    let spendable_before = provider
        .spendable_shielded_coins()
        .await
        .expect("wallet attached")
        .len();
    if spendable_before == 0 {
        eprintln!("skipping: this wallet holds no spendable shielded coins");
        return;
    }

    let recipient = midnight_wallet::address::derive_shielded(&seed, Network::Undeployed);
    let broken = provider.with_proof_provider(Arc::new(BrokenProver));

    let outcome = broken
        .transfer_shielded(
            midnight_helpers::ShieldedTokenType(midnight_helpers::HashOutput([0u8; 32])),
            1,
            &recipient,
        )
        .build()
        .await;
    match outcome {
        Ok(_) => panic!("the build must fail, because its prover panics"),
        Err(err) => eprintln!("build failed as expected: {err}"),
    }

    // `spendable_shielded_coins` filters out coins a pending build reserved, so
    // a leaked reservation shows up here as a coin that has gone missing.
    let spendable_after = broken
        .spendable_shielded_coins()
        .await
        .expect("wallet attached")
        .len();

    assert_eq!(
        spendable_after, spendable_before,
        "a build whose proof failed must release the coins it reserved"
    );
}
