//! A funded contract call must run the ZK prover over its circuit exactly once.
//!
//! Handing the funding seed to the wallet's fee-balancing fixpoint makes it
//! rebuild and re-prove the whole transaction on every iteration, circuit
//! included, and the first iteration always requests zero Dust so it always
//! loops at least once. That charged the most expensive operation in the SDK
//! three to four times per call. Nothing asserted the property, so it could
//! regress silently.
//!
//! Counting proofs needs a real prover: mock proving is not available for user
//! circuits (upstream `MockProver::check` rejects non-builtin circuits), which
//! is the whole reason the fixpoint was expensive here.
//!
//! Gated on a running devnet (`MIDNIGHT_NODE_URL`, `MIDNIGHT_INDEXER_URL`).

mod counter {
    compact_bindgen::contract!("../../devnet/contracts/counter/compiled/contract-info.json");
}

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use midnight_helpers::{
    CostModel, DefaultDB, LocalProofServer, PedersenRandomness, ProofMarker, ProofPreimageMarker,
    ProofProvider, Resolver, Signature, StdRng, Transaction,
};
use midnight_provider::{MidnightProvider, Network, WalletSeed};

const ZK_KEYS_DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../devnet/contracts/counter/compiled"
);
const DEV_WALLET_SEED: &str = "0000000000000000000000000000000000000000000000000000000000000001";

/// Wraps the real prover and records what it was asked to prove.
///
/// Proofs are split by whether the transaction carries a contract action. A
/// call legitimately produces one Dust-only proof for its fee intent; what must
/// not grow is the number of proofs covering the circuit itself.
#[derive(Default)]
struct ProofCounter {
    inner: LocalProofServer,
    with_contract_action: AtomicUsize,
    dust_only: AtomicUsize,
}

impl ProofCounter {
    fn circuit_proofs(&self) -> usize {
        self.with_contract_action.load(Ordering::Relaxed)
    }

    fn dust_only_proofs(&self) -> usize {
        self.dust_only.load(Ordering::Relaxed)
    }

    fn reset(&self) {
        self.with_contract_action.store(0, Ordering::Relaxed);
        self.dust_only.store(0, Ordering::Relaxed);
    }
}

#[async_trait::async_trait]
impl ProofProvider<DefaultDB> for ProofCounter {
    async fn prove(
        &self,
        tx: Transaction<Signature, ProofPreimageMarker, PedersenRandomness, DefaultDB>,
        rng: StdRng,
        resolver: &Resolver,
        cost_model: &CostModel,
    ) -> Transaction<Signature, ProofMarker, PedersenRandomness, DefaultDB> {
        let carries_contract_action = match &tx {
            Transaction::Standard(stx) => stx.intents.iter().any(|kv| !kv.1.actions.is_empty()),
            _ => false,
        };
        if carries_contract_action {
            self.with_contract_action.fetch_add(1, Ordering::Relaxed);
        } else {
            self.dust_only.fetch_add(1, Ordering::Relaxed);
        }
        self.inner.prove(tx, rng, resolver, cost_model).await
    }
}

#[tokio::test]
async fn a_funded_call_proves_its_circuit_exactly_once() {
    let (Ok(node_url), Ok(indexer_url)) = (
        std::env::var("MIDNIGHT_NODE_URL"),
        std::env::var("MIDNIGHT_INDEXER_URL"),
    ) else {
        eprintln!("skipping: needs MIDNIGHT_NODE_URL + MIDNIGHT_INDEXER_URL");
        return;
    };

    let counter_proofs = Arc::new(ProofCounter::default());
    let seed = WalletSeed::try_from_hex_str(DEV_WALLET_SEED).unwrap();
    let provider = MidnightProvider::new(&node_url, &indexer_url)
        .expect("provider")
        .with_proof_provider(counter_proofs.clone())
        .sync_wallet(seed, Network::Undeployed)
        .await
        .expect("sync");

    let contract = counter::Contract::deploy(&provider)
        .with_initial_state(counter::LedgerInitialState::default())
        .with_zk_config(ZK_KEYS_DIR)
        .send()
        .await
        .expect("submit deploy")
        .into_contract()
        .await
        .expect("deploy");

    let round_before = contract
        .ledger()
        .await
        .expect("ledger")
        .round()
        .expect("round");

    // Only the call is measured; deploy has its own (mock-proved) fee path.
    counter_proofs.reset();

    // Awaiting the builder submits, waits, and errors unless the call landed
    // and its fallible phase succeeded.
    let outcome = contract
        .circuits()
        .increment()
        .await
        .expect("the node must accept the call");
    eprintln!("increment() tx = {}", hex::encode(outcome.extrinsic_hash));

    let circuit_proofs = counter_proofs.circuit_proofs();
    eprintln!(
        "increment(): {circuit_proofs} circuit proof(s), {} dust-only proof(s)",
        counter_proofs.dust_only_proofs()
    );
    assert_eq!(
        circuit_proofs, 1,
        "a funded call must prove its circuit exactly once, not once per fee iteration"
    );

    // The call has to have actually run, not merely landed.
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
