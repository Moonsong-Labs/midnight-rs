//! A wallet funds a transfer, selecting and reserving under its lock, and the
//! provider then proves without it.
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
//! TTL elapses and the wallet looks poorer than it is. The same holds for a
//! funded transaction that its caller drops.
//!
//! The spendable Dust readings leave out reserved Dust, and the total counts
//! it. A new build cannot draw on reserved Dust, so a spendable reading that
//! counts it promises a fee the next build cannot pay.
//!
//! No test submits anything.
//!
//! Gated on a running devnet (`MIDNIGHT_NODE_URL`, `MIDNIGHT_INDEXER_URL`).

#[expect(dead_code, reason = "these tests need no synced_provider")]
mod common;

use std::sync::Arc;
use std::time::Duration;

use midnight_helpers::{
    CostModel, DefaultDB, LocalProofServer, ProofProvider, ProvenTransaction, Resolver, StdRng,
    UnprovenTransaction,
};
use midnight_provider::{MidnightProvider, Prover, ProviderError, TransactionRequest};
use midnight_types::{HashOutput, NIGHT, ShieldedTokenType, UnshieldedRecipient, build};
use midnight_wallet::{FundedTx, MidnightWallet, Wallet, WalletError};
use tokio::sync::Notify;

fn night() -> ShieldedTokenType {
    ShieldedTokenType(HashOutput([0u8; 32]))
}

/// Fund a tNIGHT self-transfer of 1 STAR that pays its fee in Dust, through
/// the wallet alone, on a fill from `source`. Nothing proves it, and a drop of
/// the result hands what it reserved back.
async fn fund_night_self_transfer(
    wallet: Arc<Wallet>,
    source: Arc<MidnightProvider>,
    recipient: UnshieldedRecipient,
) -> Result<FundedTx, WalletError> {
    let prover: Arc<dyn ProofProvider<DefaultDB>> = Arc::new(LocalProofServer::default());
    let context = source
        .transaction_info()
        .await
        .expect("the chain context")
        .context;
    let filled = build::fill(
        TransactionRequest::unshielded(NIGHT, 1, recipient),
        std::future::ready(Ok::<_, WalletError>(context)),
        prover,
    )
    .await?;
    wallet.fund_transaction(&*source, filled).await
}

/// The number of the wallet's tNIGHT UTXOs that a reservation holds.
fn reserved_night(balance: &midnight_wallet::WalletBalance) -> usize {
    balance
        .unshielded
        .iter()
        .filter(|u| u.token_type == NIGHT && !u.spendable)
        .count()
}

/// A prover that always fails. `ProofProvider::prove` returns a bare
/// transaction, so failing means unwinding; the build path catches that and
/// reports [`WalletError::Proving`].
struct BrokenProver;

/// Wraps the real prover and parks inside it until the test lets go, which is
/// the window the wallet must be free in.
#[derive(Default)]
struct ParkedProver {
    prover: LocalProofServer,
    started: Notify,
    release: Notify,
}

#[async_trait::async_trait]
impl ProofProvider<DefaultDB> for BrokenProver {
    async fn prove(
        &self,
        _tx: UnprovenTransaction<DefaultDB>,
        _rng: StdRng,
        _resolver: &'static Resolver,
        _cost_model: CostModel,
    ) -> ProvenTransaction<DefaultDB> {
        panic!("prover is down");
    }
}

#[async_trait::async_trait]
impl ProofProvider<DefaultDB> for ParkedProver {
    async fn prove(
        &self,
        tx: UnprovenTransaction<DefaultDB>,
        rng: StdRng,
        resolver: &'static Resolver,
        cost_model: CostModel,
    ) -> ProvenTransaction<DefaultDB> {
        self.started.notify_one();
        self.release.notified().await;
        self.prover.prove(tx, rng, resolver, cost_model).await
    }
}

/// The guarantee this split exists for: while a build is proving, the wallet is
/// readable. Before the split this read queued behind the whole proof.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_wallet_is_readable_while_a_build_proves() {
    let Some((node, indexer)) = common::devnet_urls() else {
        return;
    };
    let prover = Arc::new(ParkedProver::default());
    let provider = MidnightProvider::new(&node).with_prover(Prover::custom(prover.clone()));
    let wallet = common::synced_wallet(&provider, &indexer, common::DEV_WALLET_SEED).await;
    let recipient = wallet.shielded_recipient();
    let provider = Arc::new(provider.with_wallet(wallet));

    let building = {
        let provider = provider.clone();
        tokio::spawn(async move {
            provider
                .fill(TransactionRequest::shielded(night(), 1, recipient))
                .await
        })
    };

    // Wait until the build is inside the prover, which is where it used to be
    // holding the wallet.
    prover.started.notified().await;

    let balance = tokio::time::timeout(Duration::from_secs(10), provider.wallet().balance())
        .await
        .expect("reading the wallet must not wait for a build that is only proving");
    eprintln!(
        "read the wallet mid-proof: {} spendable dust UTXO(s)",
        balance.dust.spendable_utxos
    );

    prover.release.notify_one();
    // The build still had to reserve before it proved. The drop of its
    // envelope hands back what it took.
    building
        .await
        .expect("build task")
        .expect("the build must finish once the prover is released");
}

/// The property the single hold exists for: two fundings running at once
/// never draw the same input.
///
/// Selection reads the reserved set and the reservation writes it. Split them
/// across two acquisitions of the wallet and both fundings select before
/// either reserves, so both pick the same largest Dust UTXO and the same
/// tNIGHT UTXO. Nothing local fails: the loser is rejected on chain.
///
/// Each funding takes one tNIGHT UTXO, so the two hold two tNIGHT UTXOs only
/// when they differ. Neither funding proves or submits. A drop of each hands
/// back what it reserved.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_fundings_at_once_draw_different_inputs() {
    let Some((node, indexer)) = common::devnet_urls() else {
        return;
    };
    let source = Arc::new(MidnightProvider::new(&node));
    let wallet = common::synced_wallet(&source, &indexer, common::DEV_WALLET_SEED).await;
    let recipient = wallet.unshielded_recipient();

    // Each funding spends one tNIGHT UTXO and draws Dust for its fee, so the
    // wallet needs two of each for the two to be able to differ at all.
    let balance = wallet.balance().await;
    let spendable_dust = balance.dust.spendable_utxos;
    let night = balance
        .unshielded
        .iter()
        .filter(|u| u.token_type == NIGHT)
        .count();
    if spendable_dust < 2 || night < 2 {
        return common::skip(&format!(
            "a wallet with 2 spendable Dust UTXOs and 2 tNIGHT UTXOs \
             (this one has {spendable_dust} and {night})"
        ));
    }

    let wallet = Arc::new(wallet);
    // Real tasks, not `join!`: two futures polled by one task interleave only
    // at await points the runtime chooses, and would pass here for the wrong
    // reason.
    let first = tokio::spawn(fund_night_self_transfer(
        wallet.clone(),
        source.clone(),
        recipient,
    ));
    let second = tokio::spawn(fund_night_self_transfer(
        wallet.clone(),
        source.clone(),
        recipient,
    ));
    let first = first.await.expect("first task").expect("first funding");
    let second = second.await.expect("second task").expect("second funding");

    let held = wallet.balance().await;
    drop((first, second));
    let released = wallet.balance().await;

    assert_eq!(
        reserved_night(&held),
        2,
        "both fundings spent the same tNIGHT UTXO"
    );
    assert!(
        released.dust.spendable_utxos >= held.dust.spendable_utxos + 2,
        "both fundings drew the same Dust: {} spendable Dust UTXOs held, {} released",
        held.dust.spendable_utxos,
        released.dust.spendable_utxos
    );
}

/// A reservation takes its Dust UTXOs out of the spendable readings and leaves
/// the total alone, and the drop of the funded transaction puts them back.
///
/// The readings compare the funded state with the released one. A funding
/// syncs first, and a sync can move the block time that the balance reads
/// Dust at, so a reading from before the funding could differ for a reason
/// other than the reservation.
///
/// Nothing proves or submits.
#[tokio::test]
async fn the_spendable_dust_leaves_out_a_reservation() {
    let Some((node, indexer)) = common::devnet_urls() else {
        return;
    };
    let source = Arc::new(MidnightProvider::new(&node));
    let wallet = common::synced_wallet(&source, &indexer, common::DEV_WALLET_SEED).await;
    let recipient = wallet.unshielded_recipient();
    let wallet = Arc::new(wallet);

    let funded = fund_night_self_transfer(wallet.clone(), source, recipient)
        .await
        .expect("the dev seed pays a tNIGHT self-transfer's fee in Dust");
    let reserved = wallet.balance().await.dust;
    drop(funded);
    let released = wallet.balance().await.dust;

    assert!(
        reserved.spendable_utxos < released.spendable_utxos,
        "each Dust UTXO the funding reserved must leave the spendable count"
    );
    assert!(
        reserved.spendable_speck < released.spendable_speck,
        "the Dust the funding reserved must leave the spendable SPECK: {} reserved, {} released",
        reserved.spendable_speck,
        released.spendable_speck
    );
    assert_eq!(
        reserved.balance_speck, released.balance_speck,
        "the total must count reserved Dust"
    );
    assert_eq!(
        released.spendable_speck, released.balance_speck,
        "the drop must hand every reserved Dust UTXO back"
    );
}

/// The cost of reserving before proving: a build that fails has to give the
/// inputs back itself.
#[tokio::test]
async fn a_failed_proof_hands_the_reserved_coins_back() {
    let Some((node, indexer)) = common::devnet_urls() else {
        return;
    };
    let provider = MidnightProvider::new(&node);
    let wallet = common::synced_wallet(&provider, &indexer, common::DEV_WALLET_SEED).await;
    let recipient = wallet.shielded_recipient();
    let provider = provider.with_wallet(wallet);

    let spendable_before = provider.wallet().spendable_shielded_coins().await.len();
    if spendable_before == 0 {
        return common::skip("a spendable shielded coin");
    }

    let broken = provider.with_prover(Prover::custom(Arc::new(BrokenProver)));

    let outcome = broken
        .fill(TransactionRequest::shielded(night(), 1, recipient))
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
    let spendable_after = broken.wallet().spendable_shielded_coins().await.len();

    assert_eq!(
        spendable_after, spendable_before,
        "a build whose proof failed must release the coins it reserved"
    );
}
