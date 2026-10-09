//! A transfer build selects and reserves under the wallet, then proves without
//! it.
//!
//! The properties below follow, and each has a test here.
//!
//! Selection and reservation stay together. Two builds that run at once must
//! never draw the same input, which is what the single hold buys.
//!
//! Proving must not hold the wallet. It is the slowest step in a build and
//! reads only the build context, so holding the wallet through it makes every
//! other consumer wait on work that never needed it.
//!
//! A reservation outlives the decision that made it, so a build whose proof
//! fails has to hand its inputs back. Otherwise they stay unusable until their
//! TTL elapses and the wallet looks poorer than it is.
//!
//! The spendable Dust readings leave out reserved Dust, and the total counts
//! it. A new build cannot draw on reserved Dust, so a spendable reading that
//! counts it promises a fee the next build cannot pay.
//!
//! No test submits anything.
//!
//! Gated on a running devnet (`MIDNIGHT_NODE_URL`, `MIDNIGHT_INDEXER_URL`).

use std::sync::Arc;
use std::time::Duration;

use midnight_helpers::{DefaultDB, StdRng};
use midnight_provider::{
    LedgerVersion, MidnightProvider, NIGHT, Network, ProviderError, ShieldedTokenType, SpentInputs,
    TransferKind, TransferRequest, WalletError, WalletFacade, WalletSeed,
};
use midnight_wallet::{LocalWallet, Wallet};
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
    let provider = MidnightProvider::new(node, indexer).expect("provider");
    let wallet = Wallet::sync(&provider, seed.clone(), Network::Undeployed)
        .await
        .expect("sync");
    provider.with_wallet(LocalWallet::new(wallet))
}

fn night() -> ShieldedTokenType {
    ShieldedTokenType(midnight_provider::HashOutput([0u8; 32]))
}

/// Prepare a tNIGHT self-transfer of 1 STAR that pays its fee in Dust, through
/// the wallet's own builds, and return what it reserved. Nothing proves it.
async fn prepare_night_self_transfer(
    wallet: Arc<LocalWallet>,
    recipient: String,
) -> Result<SpentInputs, WalletError> {
    let request = TransferRequest::new(TransferKind::Unshielded {
        token_type: NIGHT,
        amount: 1,
        recipient,
        pay_fees: true,
    });
    macro_rules! prepare {
        ($ledger:ident) => {{
            use midnight_wallet_facade::$ledger::WalletBuilds;
            let prover: Arc<dyn midnight_helpers::$ledger::ProofProvider<DefaultDB>> =
                Arc::new(midnight_helpers::$ledger::LocalProofServer::default());
            wallet
                .prepare_transfer(request, prover)
                .await
                .map(|build| build.into_prepared().spent_inputs())
        }};
    }
    match wallet.ledger_version().await {
        LedgerVersion::V8 => prepare!(ledger_8),
        LedgerVersion::V9 => prepare!(ledger_9),
    }
}

/// The readings of a [`DustBalance`](midnight_wallet::DustBalance), to compare
/// two of them.
fn dust_readings(dust: &midnight_wallet::DustBalance) -> (usize, u128, u128, bool, usize) {
    // No `..`: a new field must cause a compile error here, so the comparison
    // cannot miss it.
    let midnight_wallet::DustBalance {
        spendable_utxos,
        balance_speck,
        spendable_speck,
        night_generates_dust,
        unregistered_night_utxos,
    } = dust;
    (
        *spendable_utxos,
        *balance_speck,
        *spendable_speck,
        *night_generates_dust,
        *unregistered_night_utxos,
    )
}

/// A prover that always fails. `ProofProvider::prove` returns a bare
/// transaction, so failing means unwinding; the build path catches that and
/// reports [`WalletError::Proving`].
struct BrokenProver;

/// Wraps the real prover and parks inside it until the test lets go, which is
/// the window the wallet must be free in.
#[derive(Default)]
struct ParkedProver {
    ledger_8: midnight_helpers::ledger_8::LocalProofServer,
    ledger_9: midnight_helpers::ledger_9::LocalProofServer,
    started: Notify,
    release: Notify,
}

/// Each prover for one generation; the bodies are the same on each.
macro_rules! test_provers {
    ($ledger:ident) => {
        #[async_trait::async_trait]
        impl midnight_helpers::$ledger::ProofProvider<DefaultDB> for BrokenProver {
            async fn prove(
                &self,
                _tx: midnight_helpers::$ledger::Transaction<
                    midnight_helpers::$ledger::Signature,
                    midnight_helpers::$ledger::ProofPreimageMarker,
                    midnight_helpers::$ledger::PedersenRandomness,
                    DefaultDB,
                >,
                _rng: StdRng,
                _resolver: &'static midnight_helpers::$ledger::Resolver,
                _cost_model: midnight_helpers::$ledger::CostModel,
            ) -> midnight_helpers::$ledger::Transaction<
                midnight_helpers::$ledger::Signature,
                midnight_helpers::$ledger::ProofMarker,
                midnight_helpers::$ledger::PedersenRandomness,
                DefaultDB,
            > {
                panic!("prover is down");
            }
        }

        #[async_trait::async_trait]
        impl midnight_helpers::$ledger::ProofProvider<DefaultDB> for ParkedProver {
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
                self.started.notify_one();
                self.release.notified().await;
                self.$ledger.prove(tx, rng, resolver, cost_model).await
            }
        }
    };
}

test_provers!(ledger_8);
test_provers!(ledger_9);

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
        .release(&SpentInputs::from(&result))
        .await
        .expect("release the reservation");
}

/// The property the single hold exists for: two preparations running at once
/// never draw the same input.
///
/// Selection reads the reserved set and the reservation writes it. Split them
/// across two acquisitions of the wallet and both preparations select before
/// either reserves, so both pick the same largest Dust UTXO and the same
/// tNIGHT UTXO. Nothing local fails: the loser is rejected on chain.
///
/// This drives [`LocalWallet`] directly rather than the provider. Every
/// provider build path resyncs first, and the resync mutex serializes two
/// builds long before they reach the wallet, which hides the race the hold
/// exists to stop.
///
/// Neither preparation proves or submits. Both release what they reserved.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_preparations_at_once_draw_different_inputs() {
    let (node, indexer) = devnet_or_skip!();
    let seed = WalletSeed::try_from_hex_str(DEV_WALLET_SEED).expect("dev seed");
    let address = midnight_wallet::address::derive_unshielded(&seed, Network::Undeployed);

    let source = MidnightProvider::new(&node, &indexer).expect("provider");
    let wallet = Wallet::sync(&source, seed.clone(), Network::Undeployed)
        .await
        .expect("sync");

    // Each preparation spends one tNIGHT UTXO and draws Dust for its fee, so
    // the wallet needs two of each for the two to be able to differ at all.
    let spendable_dust = wallet.balance().dust.spendable_utxos;
    let night = wallet
        .unshielded_utxos()
        .iter()
        .filter(|u| u.is_night())
        .count();
    if spendable_dust < 2 || night < 2 {
        if std::env::var_os("MIDNIGHT_E2E").is_some() {
            panic!(
                "2 spendable Dust UTXOs and 2 tNIGHT UTXOs are missing under make test-e2e: \
                 this wallet has {spendable_dust} and {night}"
            );
        }
        eprintln!(
            "skipping: needs 2 spendable Dust UTXOs and 2 tNIGHT UTXOs, \
             this wallet has {spendable_dust} and {night}"
        );
        return;
    }

    let wallet = Arc::new(LocalWallet::new(wallet));
    // Real tasks, not `join!`: two futures polled by one task interleave only
    // at await points the runtime chooses, and would pass here for the wrong
    // reason.
    let first = tokio::spawn(prepare_night_self_transfer(wallet.clone(), address.clone()));
    let second = tokio::spawn(prepare_night_self_transfer(wallet.clone(), address.clone()));
    let first = first.await.expect("first task").expect("first preparation");
    let second = second
        .await
        .expect("second task")
        .expect("second preparation");

    let (first_dust, second_dust) = (&first.dust, &second.dust);
    assert!(
        !first_dust.iter().any(|n| second_dust.contains(n)),
        "both preparations drew the same Dust: {first_dust:?} and {second_dust:?}"
    );
    assert!(
        !first
            .unshielded
            .iter()
            .any(|k| second.unshielded.contains(k)),
        "both preparations spent the same tNIGHT UTXO: {:?} and {:?}",
        first.unshielded,
        second.unshielded
    );

    wallet.release(&first).await;
    wallet.release(&second).await;
}

/// A reservation takes its Dust UTXOs out of the spendable readings and leaves
/// the total alone, and a release puts them back.
///
/// This drives [`LocalWallet`] directly rather than the provider. A provider
/// build resyncs first, and a resync can move the block time that the balance
/// reads Dust at, so the readings would differ for a reason other than the
/// reservation.
///
/// Nothing proves or submits.
#[tokio::test]
async fn the_spendable_dust_leaves_out_a_reservation() {
    let (node, indexer) = devnet_or_skip!();
    let seed = WalletSeed::try_from_hex_str(DEV_WALLET_SEED).expect("dev seed");
    let address = midnight_wallet::address::derive_unshielded(&seed, Network::Undeployed);
    let source = MidnightProvider::new(&node, &indexer).expect("provider");
    let wallet = Arc::new(LocalWallet::new(
        Wallet::sync(&source, seed, Network::Undeployed)
            .await
            .expect("sync"),
    ));

    let before = wallet.balance().await.dust;
    let spent = prepare_night_self_transfer(wallet.clone(), address)
        .await
        .expect("the dev seed pays a tNIGHT self-transfer's fee in Dust");
    let reserved = wallet.balance().await.dust;
    wallet.release(&spent).await;
    let released = wallet.balance().await.dust;

    assert_eq!(
        reserved.spendable_utxos + spent.dust.len(),
        before.spendable_utxos,
        "each Dust UTXO the build reserved must leave the spendable count"
    );
    assert!(
        reserved.spendable_speck < before.spendable_speck,
        "the Dust the build reserved must leave the spendable SPECK: {} before, {} reserved",
        before.spendable_speck,
        reserved.spendable_speck
    );
    assert_eq!(
        reserved.balance_speck, before.balance_speck,
        "the total must count reserved Dust"
    );
    assert_eq!(
        dust_readings(&released),
        dust_readings(&before),
        "a release must hand the reserved Dust back"
    );
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
        if std::env::var_os("MIDNIGHT_E2E").is_some() {
            panic!("spendable shielded coins are missing under make test-e2e");
        }
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
