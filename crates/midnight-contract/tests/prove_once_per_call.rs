//! A funded contract call must run the ZK prover over its circuit exactly once.
//!
//! A fee balanced with real proofs proves the whole transaction again on every
//! round, circuit included. That charges the most expensive operation in the
//! SDK several times per call, and no other test catches it.
//!
//! Counting proofs needs a real prover: mock proving is not available for user
//! circuits (upstream `MockProver::check` rejects non-builtin circuits).
//!
//! A call whose local verifier key is not the deployed key must make no
//! proof: the call must refuse before the build.
//!
//! The same prover then fails on purpose. A call, a deploy and a maintenance
//! update must each return the failure typed, as `WalletError::Proving` inside
//! `ContractError::Provider`, so that a caller can match it.
//!
//! Gated on a running devnet (`MIDNIGHT_NODE_URL`, `MIDNIGHT_INDEXER_URL`).
//! Under `make test-e2e`, which sets `MIDNIGHT_E2E`, a missing URL panics.

mod common;

mod counter {
    compact_bindgen::contract!("../../devnet/contracts/counter/compiled/compiler/analyzed-ir.sexp");
}

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use midnight_contract::{
    ContractError, FsZkConfigProvider, KeyMismatch, ZkConfigError, ZkConfigProvider,
};
use midnight_helpers::ledger_9::{
    CostModel, ProofProvider, ProvenTransaction, Resolver, Transaction, UnprovenTransaction,
};
use midnight_helpers::{DefaultDB, StdRng};
use midnight_provider::{MidnightProvider, ProviderError, WalletError};

/// The message of every proof that [`ProofCounter`] fails.
const MARKER: &str = "the test prover fails on purpose";

/// Wraps the real prover and records what it was asked to prove.
///
/// Proofs are split by whether the transaction carries a contract action. A
/// call legitimately produces one Dust-only proof for its fee intent; what must
/// not grow is the number of proofs covering the circuit itself.
///
/// While `fail` is set, every proof fails with [`MARKER`].
struct ProofCounter {
    /// The SDK's prover, which proves the ZKIR v3 circuits of the counter.
    prover: Arc<dyn ProofProvider<DefaultDB>>,
    with_contract_action: AtomicUsize,
    dust_only: AtomicUsize,
    fail: AtomicBool,
}

/// The counter's artifacts, with the `increment_by` verifier key in place of
/// the `increment` key.
struct SwappedKey(FsZkConfigProvider);

impl ZkConfigProvider for SwappedKey {
    fn prover_key(&self, circuit: &str) -> Result<Vec<u8>, ZkConfigError> {
        self.0.prover_key(circuit)
    }

    fn verifier_key(&self, circuit: &str) -> Result<Vec<u8>, ZkConfigError> {
        match circuit {
            "increment" => self.0.verifier_key("increment_by"),
            other => self.0.verifier_key(other),
        }
    }

    fn zkir(&self, circuit: &str) -> Result<Vec<u8>, ZkConfigError> {
        self.0.zkir(circuit)
    }
}

impl ProofCounter {
    fn new(prover: Arc<dyn ProofProvider<DefaultDB>>) -> Self {
        Self {
            prover,
            with_contract_action: AtomicUsize::default(),
            dust_only: AtomicUsize::default(),
            fail: AtomicBool::default(),
        }
    }

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
        tx: UnprovenTransaction<DefaultDB>,
        rng: StdRng,
        resolver: &'static Resolver,
        cost_model: CostModel,
    ) -> ProvenTransaction<DefaultDB> {
        if self.fail.load(Ordering::Relaxed) {
            // `resume_unwind`, not `panic!`: it skips the panic hook, so the
            // log shows no panic for an expected failure.
            std::panic::resume_unwind(Box::new(MARKER.to_string()));
        }
        let carries_contract_action = match &tx {
            Transaction::Standard(stx) => stx.intents.iter().any(|kv| !kv.1.actions.is_empty()),
            _ => false,
        };
        if carries_contract_action {
            self.with_contract_action.fetch_add(1, Ordering::Relaxed);
        } else {
            self.dust_only.fetch_add(1, Ordering::Relaxed);
        }
        self.prover.prove(tx, rng, resolver, cost_model).await
    }
}

#[tokio::test]
async fn a_funded_call_proves_its_circuit_exactly_once() {
    let Some((node_url, indexer_url)) = common::devnet_urls() else {
        return;
    };

    let provider = MidnightProvider::new(&node_url);
    let counter_proofs = Arc::new(ProofCounter::new(provider.proof_provider()));
    let provider = provider.with_proof_provider(counter_proofs.clone());
    let wallet = common::synced_wallet(&provider, &indexer_url, common::DEV_WALLET_SEED).await;
    let provider = provider.with_wallet(wallet);

    let authority = midnight_contract::SigningKey::sample(rand::thread_rng());
    let contract = counter::Contract::deploy(&provider)
        .with_initial_state(counter::LedgerInitialState::default())
        .with_zk_config(counter::ZK_CONFIG_DIR)
        .with_maintenance_authority(vec![authority.verifying_key()], 1)
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

    // The wait errors unless the call landed and its fallible phase
    // succeeded.
    let outcome = contract
        .circuits()
        .increment()
        .send()
        .await
        .expect("submit increment")
        .wait_finalized()
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

    counter_proofs.reset();
    let swapped = SwappedKey(FsZkConfigProvider::new(counter::ZK_CONFIG_DIR));
    let refused = counter::Contract::at(&provider, contract.address())
        .with_zk_config(Arc::new(swapped))
        .build()
        .circuits()
        .increment()
        .send()
        .await;
    match refused {
        Err(ContractError::VerifierKeyMismatch {
            reason: KeyMismatch::Mismatched,
            ..
        }) => {}
        other => panic!("expected a verifier key mismatch, got {other:?}"),
    }
    assert_eq!(
        (
            counter_proofs.circuit_proofs(),
            counter_proofs.dust_only_proofs()
        ),
        (0, 0),
        "a call refused for its verifier key must make no proof"
    );

    // No failed build submits, so the dev seed sees no spend. The call fails
    // before it selects Dust. A failed deploy or maintenance build releases its
    // Dust. So each build below still reaches the prover.
    counter_proofs.fail.store(true, Ordering::Relaxed);
    expect_proving_failure("increment()", contract.circuits().increment().send().await);
    expect_proving_failure(
        "a deploy",
        counter::Contract::deploy(&provider)
            .with_initial_state(counter::LedgerInitialState::default())
            .with_zk_config(counter::ZK_CONFIG_DIR)
            .send()
            .await,
    );
    expect_proving_failure(
        "a maintenance update",
        contract
            .maintenance()
            .replace_authority(vec![authority.verifying_key()], 1)
            .prepare()
            .await
            .expect("prepare maintenance")
            .sign(0, &authority)
            .build()
            .await,
    );
}

/// Assert that `outcome` is the failure of a [`ProofCounter`] proof, typed.
fn expect_proving_failure<T>(what: &str, outcome: Result<T, ContractError>) {
    match outcome {
        Err(ContractError::Provider(ProviderError::Wallet(WalletError::Proving(msg)))) => {
            assert!(
                msg.contains(MARKER),
                "{what}: expected the test prover's failure, got {msg}"
            );
        }
        Err(other) => panic!("{what}: expected a typed proving failure, got {other:?}"),
        Ok(_) => panic!("{what} must fail, because its prover fails"),
    }
}
