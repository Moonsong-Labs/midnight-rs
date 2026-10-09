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
//! Right after the call, before any resync, its fee Dust must still be
//! reserved: the submit carries the reservation, and only a definitive
//! rejection hands it back.
//!
//! The same prover then fails on purpose. A call, a deploy and a maintenance
//! update must each return the failure typed, as `WalletError::Proving` inside
//! `ContractError::Provider`, so that a caller can match it.
//!
//! Last, a fresh wallet with no Dust calls the contract. The call must fail
//! with `WalletError::InsufficientDust` before the prover sees its circuit.
//! Then the same wallet builds a Dustless call, which must reach the prover.
//!
//! Gated on a running devnet (`MIDNIGHT_NODE_URL`, `MIDNIGHT_INDEXER_URL`).
//! Under `make test-e2e`, which sets `MIDNIGHT_E2E`, a missing URL panics.

mod counter {
    compact_bindgen::contract!("../../devnet/contracts/counter/compiled/compiler/analyzed-ir.sexp");
}

use midnight_wallet::{LocalWallet, Wallet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use midnight_contract::ContractError;
use midnight_helpers::{DefaultDB, StdRng};
use midnight_provider::{
    DustlessBuilder, MidnightProvider, Network, ProviderError, WalletError, WalletSeed,
};

const ZK_KEYS_DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../devnet/contracts/counter/compiled"
);
const DEV_WALLET_SEED: &str = "0000000000000000000000000000000000000000000000000000000000000001";
/// The message of every proof that [`ProofCounter`] fails.
const MARKER: &str = "the test prover fails on purpose";

/// Wraps the real prover and records what it was asked to prove.
///
/// Proofs are split by whether the transaction carries a contract action. A
/// call legitimately produces one Dust-only proof for its fee intent; what must
/// not grow is the number of proofs covering the circuit itself.
///
/// While `fail` is set, every proof fails with [`MARKER`].
#[derive(Default)]
struct ProofCounter {
    ledger_8: midnight_helpers::ledger_8::LocalProofServer,
    ledger_9: midnight_helpers::ledger_9::LocalProofServer,
    with_contract_action: AtomicUsize,
    dust_only: AtomicUsize,
    fail: AtomicBool,
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

/// The counter for each generation; the body is the same on each.
macro_rules! proof_counter {
    ($ledger:ident) => {
        #[async_trait::async_trait]
        impl midnight_helpers::$ledger::ProofProvider<DefaultDB> for ProofCounter {
            async fn prove(
                &self,
                tx: midnight_helpers::$ledger::Transaction<
                    midnight_helpers::$ledger::Signature,
                    midnight_helpers::$ledger::ProofPreimageMarker,
                    midnight_helpers::$ledger::PedersenRandomness,
                    DefaultDB,
                >,
                rng: StdRng,
                resolver: &'static midnight_helpers::$ledger::Resolver,
                cost_model: midnight_helpers::$ledger::CostModel,
            ) -> midnight_helpers::$ledger::Transaction<
                midnight_helpers::$ledger::Signature,
                midnight_helpers::$ledger::ProofMarker,
                midnight_helpers::$ledger::PedersenRandomness,
                DefaultDB,
            > {
                if self.fail.load(Ordering::Relaxed) {
                    // `resume_unwind`, not `panic!`: it skips the panic hook,
                    // so the log shows no panic for an expected failure.
                    std::panic::resume_unwind(Box::new(MARKER.to_string()));
                }
                let carries_contract_action = match &tx {
                    midnight_helpers::$ledger::Transaction::Standard(stx) => {
                        stx.intents.iter().any(|kv| !kv.1.actions.is_empty())
                    }
                    _ => false,
                };
                if carries_contract_action {
                    self.with_contract_action.fetch_add(1, Ordering::Relaxed);
                } else {
                    self.dust_only.fetch_add(1, Ordering::Relaxed);
                }
                self.$ledger.prove(tx, rng, resolver, cost_model).await
            }
        }
    };
}

proof_counter!(ledger_8);
proof_counter!(ledger_9);

#[tokio::test]
async fn a_funded_call_proves_its_circuit_exactly_once() {
    let (Ok(node_url), Ok(indexer_url)) = (
        std::env::var("MIDNIGHT_NODE_URL"),
        std::env::var("MIDNIGHT_INDEXER_URL"),
    ) else {
        if std::env::var_os("MIDNIGHT_E2E").is_some() {
            panic!("MIDNIGHT_NODE_URL or MIDNIGHT_INDEXER_URL is missing under make test-e2e");
        }
        eprintln!("skipping: needs MIDNIGHT_NODE_URL + MIDNIGHT_INDEXER_URL");
        return;
    };

    let counter_proofs = Arc::new(ProofCounter::default());
    let seed = WalletSeed::try_from_hex_str(DEV_WALLET_SEED).unwrap();
    let provider = MidnightProvider::new(&node_url, &indexer_url)
        .expect("provider")
        .with_proof_provider(counter_proofs.clone());
    let wallet = Wallet::sync(&provider, seed, Network::Undeployed)
        .await
        .expect("sync");
    let provider = provider.with_wallet(LocalWallet::new(wallet));

    let authority = midnight_contract::SigningKey::sample(rand::thread_rng());
    let contract = counter::Contract::deploy(&provider)
        .with_initial_state(counter::LedgerInitialState::default())
        .with_zk_config(ZK_KEYS_DIR)
        .with_maintenance_authority(vec![authority.verifying_key()], 1)
        .send()
        .await
        .expect("submit deploy")
        .into_contract()
        .await
        .expect("deploy");

    // The indexer serves the deploy now, so a resync sees its Dust spend and
    // clears its reservation. Only the call's own reservation can then keep
    // Dust reserved below.
    provider.resync_wallet().await.expect("resync");
    let dust = provider.balance().await.expect("balance").dust;
    assert_eq!(
        dust.spendable_speck, dust.balance_speck,
        "no reservation may be live before the call, got {dust:?}"
    );

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

    // No resync has seen the call's spend yet, so its fee Dust must still be
    // reserved. A submit that hands the inputs back on success frees it, and
    // the next build would draw the in-flight Dust again (error 196).
    let dust = provider.balance().await.expect("balance").dust;
    assert!(
        dust.spendable_speck < dust.balance_speck,
        "the call's fee Dust must stay reserved after a successful submit, got {dust:?}"
    );

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

    // No failed build submits, so the dev seed sees no spend. The call fails
    // before it selects Dust. Its Dust check needs only spendable Dust above
    // zero. The deploy below needs spendable Dust before its proof too. A failed
    // deploy or maintenance build releases its Dust. So each build below still
    // reaches the prover.
    counter_proofs.fail.store(true, Ordering::Relaxed);
    expect_proving_failure("increment()", contract.circuits().increment().await);
    expect_proving_failure(
        "a deploy",
        counter::Contract::deploy(&provider)
            .with_initial_state(counter::LedgerInitialState::default())
            .with_zk_config(ZK_KEYS_DIR)
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

    // Clear the prover fault, so that the Dustless call below can prove its
    // circuit and build.
    counter_proofs.fail.store(false, Ordering::Relaxed);
    let fresh = MidnightProvider::new(&node_url, &indexer_url)
        .expect("provider")
        .with_proof_provider(counter_proofs.clone());
    let wallet = Wallet::sync(&fresh, unused_seed(), Network::Undeployed)
        .await
        .expect("sync the fresh wallet");
    let fresh = fresh.with_wallet(LocalWallet::new(wallet));
    let unfunded = counter::Contract::at(&fresh, contract.address())
        .with_zk_config(ZK_KEYS_DIR)
        .build();
    counter_proofs.reset();
    match unfunded.circuits().increment().await {
        Err(ContractError::Provider(ProviderError::Wallet(WalletError::InsufficientDust {
            required: None,
            available: 0,
        }))) => {}
        Err(other) => panic!("a call with no Dust must fail before its proof, got {other:?}"),
        Ok(_) => panic!("a call from a wallet with no Dust must fail"),
    }
    assert_eq!(
        counter_proofs.circuit_proofs(),
        0,
        "a call with no spendable Dust must not prove its circuit"
    );
    unfunded
        .circuits()
        .increment()
        .without_dust()
        .await
        .expect("a Dustless call from a wallet with no Dust must build");
    assert_eq!(
        counter_proofs.circuit_proofs(),
        1,
        "a Dustless call must reach the prover with no Dust"
    );
}

/// A seed that no earlier run has used, so its wallet holds nothing.
fn unused_seed() -> WalletSeed {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    WalletSeed::try_from_hex_str(&format!("{nonce:064x}")).expect("seed from nonce")
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
