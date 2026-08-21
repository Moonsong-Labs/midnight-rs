//! A transfer build selects and reserves under the wallet, then proves without
//! it.
//!
//! Two properties follow, and each has a test here.
//!
//! Proving must not hold the wallet. It is the slowest step in a build and
//! reads only the build context, so holding the wallet through it makes every
//! other consumer wait on work that never needed it.
//!
//! A reservation now outlives the decision that made it, so a build that ends
//! before it finishes has to hand its inputs back. Otherwise they stay
//! unusable until their TTL elapses and the wallet looks poorer than it is.
//!
//! Neither test submits anything.
//!
//! Gated on a running devnet (`MIDNIGHT_NODE_URL`, `MIDNIGHT_INDEXER_URL`).

use std::sync::Arc;
use std::time::Duration;

use midnight_helpers::{
    CostModel, DefaultDB, LocalProofServer, PedersenRandomness, ProofMarker, ProofPreimageMarker,
    ProofProvider, Resolver, Signature, StdRng, Transaction,
};
use midnight_provider::{MidnightProvider, Network, ProviderError, WalletError, WalletSeed};
use tokio::sync::Notify;

const DEV_WALLET_SEED: &str = "0000000000000000000000000000000000000000000000000000000000000001";

macro_rules! devnet_or_skip {
    () => {{
        let (Ok(node), Ok(indexer)) = (
            std::env::var("MIDNIGHT_NODE_URL"),
            std::env::var("MIDNIGHT_INDEXER_URL"),
        ) else {
            eprintln!("skipping: needs MIDNIGHT_NODE_URL + MIDNIGHT_INDEXER_URL");
            return;
        };
        (node, indexer)
    }};
}

async fn synced_provider(node: &str, indexer: &str, seed: &WalletSeed) -> MidnightProvider {
    MidnightProvider::new(node, indexer)
        .expect("provider")
        .sync_wallet(seed.clone(), Network::Undeployed)
        .await
        .expect("sync")
}

fn night() -> midnight_helpers::ShieldedTokenType {
    midnight_helpers::ShieldedTokenType(midnight_helpers::HashOutput([0u8; 32]))
}

/// A prover that always fails. `ProofProvider::prove` returns a bare
/// transaction, so failing means unwinding; the build path catches that and
/// reports [`WalletError::Proving`].
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

/// Wraps the real prover and parks inside it until the test lets go, which is
/// the window the wallet must be free in.
#[derive(Default)]
struct ParkedProver {
    inner: LocalProofServer,
    started: Notify,
    release: Notify,
}

#[async_trait::async_trait]
impl ProofProvider<DefaultDB> for ParkedProver {
    async fn prove(
        &self,
        tx: Transaction<Signature, ProofPreimageMarker, PedersenRandomness, DefaultDB>,
        rng: StdRng,
        resolver: &Resolver,
        cost_model: &CostModel,
    ) -> Transaction<Signature, ProofMarker, PedersenRandomness, DefaultDB> {
        self.started.notify_one();
        self.release.notified().await;
        self.inner.prove(tx, rng, resolver, cost_model).await
    }
}

/// The guarantee this split exists for: while a build is proving, the wallet is
/// readable. Before the split this read queued behind the whole proof.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_wallet_is_readable_while_a_build_proves() {
    let (node, indexer) = devnet_or_skip!();
    let seed = WalletSeed::try_from_hex_str(DEV_WALLET_SEED).expect("dev seed");

    let prover = Arc::new(ParkedProver::default());
    let provider = Arc::new(
        synced_provider(&node, &indexer, &seed)
            .await
            .with_proof_provider(prover.clone()),
    );

    let recipient = midnight_wallet::address::derive_shielded(&seed, Network::Undeployed);
    let building = {
        let provider = provider.clone();
        tokio::spawn(async move {
            provider
                .transfer_shielded(night(), 1, &recipient)
                .build()
                .await
        })
    };

    // Wait until the build is inside the prover, which is where it used to be
    // holding the wallet.
    prover.started.notified().await;

    let balance = tokio::time::timeout(Duration::from_secs(10), provider.balance())
        .await
        .expect("reading the wallet must not wait for a build that is only proving")
        .expect("wallet attached");
    eprintln!(
        "read the wallet mid-proof: {} spendable dust UTXO(s)",
        balance.dust.spendable_utxos
    );

    prover.release.notify_one();
    let result = building
        .await
        .expect("build task")
        .expect("the build must finish once the prover is released");

    // The build still had to reserve before it proved, so release what it took.
    provider
        .release_pending(&result)
        .await
        .expect("release the reservation");
}

/// The cost of reserving before proving: a build that fails has to give the
/// inputs back itself.
#[tokio::test]
async fn a_failed_proof_hands_the_reserved_coins_back() {
    let (node, indexer) = devnet_or_skip!();
    let seed = WalletSeed::try_from_hex_str(DEV_WALLET_SEED).expect("dev seed");
    let provider = synced_provider(&node, &indexer, &seed).await;

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
        .transfer_shielded(night(), 1, &recipient)
        .build()
        .await;

    // Match the proving failure specifically. Any earlier error (coin
    // selection, fee balancing) happens before a reservation exists, so it
    // would leave the assertion below true without exercising the cleanup.
    match outcome {
        Ok(_) => panic!("the build must fail, because its prover panics"),
        Err(ProviderError::Wallet(WalletError::Proving(msg))) => {
            assert!(
                msg.contains("prover is down"),
                "expected our prover's panic, got {msg}"
            );
        }
        Err(other) => panic!("expected a proving failure, got {other}"),
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
