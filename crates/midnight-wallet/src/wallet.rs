//! The state of a synced wallet on whichever ledger generation its chain
//! runs, and the sync that carries it across a hard fork.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use midnight_helpers::DefaultDB;
use midnight_indexer_client::SubscriptionClient;
use midnight_types::chain_pin::ChainPin;
use midnight_types::hd::Seed;
use midnight_types::{
    ChainParameters, CoinInfo, CoinPublicKey, EncryptionPublicKey, LedgerVersion,
    SpendableShieldedCoin, SpentInputs, SyncCursors, TrackedUtxo, WalletBalance, WalletError,
};
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::handle::Identity;
use crate::replay::{
    UnshieldedReplay, latest_block, ledger_of, progress_cancelled, replay_unshielded_events,
    send_progress,
};
use crate::storage::wallet_storage_id;
use crate::sync::SyncPlan;
use crate::{SyncProgress, ledger_8, ledger_9};

/// The synced ledger state behind a [`Wallet`](crate::Wallet) handle, in the
/// generation its chain runs ([`Self::ledger_version`]).
pub(crate) enum WalletState {
    Ledger8(ledger_8::Wallet),
    Ledger9(ledger_9::Wallet),
}

/// Evaluate `$body` with `$w` bound to the state of whichever generation
/// `$state` holds.
macro_rules! each {
    ($state:expr, $w:ident => $body:expr) => {
        match $state {
            WalletState::Ledger8($w) => $body,
            WalletState::Ledger9($w) => $body,
        }
    };
}

impl WalletState {
    /// The ledger generation this wallet's state is in.
    pub(crate) fn ledger_version(&self) -> LedgerVersion {
        match self {
            WalletState::Ledger8(_) => LedgerVersion::V8,
            WalletState::Ledger9(_) => LedgerVersion::V9,
        }
    }

    /// The identity the sync gave this wallet. A fork crossing keeps it.
    pub(crate) fn identity(&self) -> &Arc<Identity> {
        each!(self, w => w.identity())
    }

    /// The sync that [`crate::WalletBuilder::sync`] and
    /// [`crate::WalletBuilder::sync_with_progress`] run.
    ///
    /// Reads the generation the chain runs from its latest block, resumes
    /// from a snapshot under `storage_dir` when there is one, and replays the
    /// three event streams, crossing each hard fork they meet. Checkpoints
    /// Dust progress to disk periodically, so an interrupted sync resumes
    /// where it left off.
    pub(crate) async fn sync_inner(
        plan: SyncPlan,
        progress: Option<mpsc::Sender<SyncProgress>>,
    ) -> Result<Self, WalletError> {
        let sync = ChainSync {
            indexer_url: &plan.indexer_url,
            identity: &plan.identity,
            storage_dir: plan.storage_dir.as_deref(),
            save: true,
            chain_pin: plan.chain_pin,
        };
        // A chain can fork between the read of its latest block and the
        // replay of its events. The replay then meets the later generation,
        // and a second attempt reads the block again and starts from there.
        match sync_once(sync.clone(), progress.clone()).await {
            Err(WalletError::LedgerMismatch { expected, found }) if found > expected => {
                info!(%found, "the chain moved to a later ledger during the sync; syncing again");
                sync_once(sync, progress).await
            }
            result => result,
        }
    }

    /// Whether the dust state has been synced (required for transaction
    /// building).
    pub(crate) fn dust_synced(&self) -> bool {
        each!(self, w => w.dust_synced())
    }

    /// Hand back the inputs a build reserved, because that build will never
    /// reach the chain.
    ///
    /// Reserving on build stops a later build re-selecting the same inputs, so
    /// a transaction that is rejected at submit, or built and then abandoned,
    /// holds its coins until the TTL window elapses. Releasing returns them at
    /// once. Pass what the build reported spending.
    ///
    /// Only call this for a transaction that cannot land. Releasing one that is
    /// still in flight lets a later build re-select the same inputs, and the
    /// loser is rejected on chain.
    pub(crate) fn release(&mut self, spent: &SpentInputs) {
        each!(self, w => w.release(spent))
    }

    /// Where this wallet's snapshot lives, when it persists one.
    ///
    /// An error that tells a reader to remove the snapshot has to name it, so
    /// this is the path that goes in the message.
    pub(crate) fn snapshot_dir(&self) -> Option<PathBuf> {
        each!(self, w => w.snapshot_dir())
    }

    /// The finalized block this wallet is pinned to, held in memory.
    ///
    /// A resume checks the pin on disk; a wallet that stays attached has to
    /// check this one, because its cursors go just as stale when the chain is
    /// replaced underneath it.
    pub(crate) fn chain_pin(&self) -> Option<&ChainPin> {
        each!(self, w => w.chain_pin())
    }

    /// Move the pin to the block a fresh check saw, so it stays inside an
    /// archive's retention window rather than ageing out of it.
    pub(crate) fn set_chain_pin(&mut self, pin: ChainPin) {
        each!(self, w => w.set_chain_pin(pin))
    }

    /// See [`WalletFacade::sync_cursors`](crate::WalletFacade::sync_cursors).
    pub(crate) fn sync_cursors(&self) -> SyncCursors {
        each!(self, w => w.sync_cursors())
    }

    /// See [`WalletFacade::shielded_public_keys`](crate::WalletFacade::shielded_public_keys).
    pub(crate) fn shielded_public_keys(&self) -> (CoinPublicKey, EncryptionPublicKey) {
        each!(self, w => w.shielded_public_keys())
    }

    /// The unshielded UTXOs this wallet tracks.
    pub(crate) fn unshielded_utxos(&self) -> &[TrackedUtxo] {
        each!(self, w => w.unshielded_utxos())
    }

    /// The chain's Dust and TTL parameters, as this wallet last synced them.
    pub(crate) fn parameters(&self) -> ChainParameters {
        each!(self, w => w.chain_parameters())
    }

    pub(crate) fn balance(&self) -> WalletBalance {
        each!(self, w => w.balance())
    }

    /// See [`WalletFacade::spendable_shielded_coins`](crate::WalletFacade::spendable_shielded_coins).
    pub(crate) fn spendable_shielded_coins(&self) -> Vec<SpendableShieldedCoin> {
        each!(self, w => w.spendable_shielded_coins())
    }

    /// Register coins this wallet owns but cannot discover, so a replay
    /// claims them without decrypting anything. Persist once.
    /// [`WalletFacade::watch_for_coins`](crate::WalletFacade::watch_for_coins)
    /// says which coins those are.
    ///
    /// Registration records the coin's commitment under this wallet's coin
    /// public key. A replay that meets the matching output claims the coin
    /// from that record.
    ///
    /// **Registration alone does not recover a coin whose output the wallet
    /// already replayed past.** A replay that meets an output it cannot claim
    /// collapses that Merkle leaf, and a resync resumes from the cursor
    /// instead of revisiting it. So the [`Wallet`](crate::Wallet) handle
    /// registers, then replays the stream from its first event.
    pub(crate) fn watch_for_coins(
        &mut self,
        coins: impl IntoIterator<Item = CoinInfo>,
    ) -> Result<(), WalletError> {
        each!(self, w => w.watch_for_coins(coins))
    }

    /// Drop the registrations [`Self::watch_for_coins`] made, for coins that
    /// turned out to be wrong. Persist once.
    ///
    /// A registration whose `CoinInfo` does not match the coin an on-chain
    /// output commits to never matches anything, so it would otherwise ride
    /// along on every later replay.
    ///
    /// This drops the registration only. A coin the wallet already claimed is
    /// untouched, and stays spendable: it is held now, not watched for.
    /// Forgetting a coin that was never registered changes no registration.
    pub(crate) fn forget_coins(
        &mut self,
        coins: impl IntoIterator<Item = CoinInfo>,
    ) -> Result<(), WalletError> {
        each!(self, w => w.forget_coins(coins))
    }

    /// Snapshot the inputs of a resync's replay phase. See [`ResyncPlan`].
    pub(crate) fn resync_plan(&self) -> ResyncPlan {
        let kind = match self {
            WalletState::Ledger8(w) => ResyncPlanKind::Ledger8 {
                plan: w.resync_plan(),
                crossing: Crossing {
                    identity: Arc::clone(w.identity()),
                    storage_dir: w.storage_dir().map(Path::to_path_buf),
                    chain_pin: w.chain_pin().cloned(),
                },
            },
            WalletState::Ledger9(w) => ResyncPlanKind::Ledger9(w.resync_plan()),
        };
        ResyncPlan {
            indexer_url: each!(self, w => w.indexer_url()).to_string(),
            kind,
        }
    }

    /// Apply validated resync results to `self`, and persist when they change
    /// durable state. A commit also persists when the last save failed.
    /// Separate from [`ResyncPlan::run`], which performs the I/O and
    /// validation, so this sequence is unit-testable without an indexer and
    /// so lock-sharing callers can scope their write lock to this call alone.
    ///
    /// Commit-time semantics, relevant when the wallet was mutated between
    /// [`Self::resync_plan`] and this call (callers must still prevent
    /// *concurrent resyncs*; see [`ResyncPlan::run`]):
    ///
    /// - Replay-derived state (`dust_wallet`, `zswap_state`,
    ///   `unshielded_utxos`) and the sync cursors are overwritten. With
    ///   resyncs serialized, the only state an overwrite could clobber is a
    ///   coin registration (see below): transfer builds record their
    ///   in-flight spends in the separate pending set and never touch
    ///   confirmed state.
    /// - Coin registrations ([`Self::watch_for_coins`]) live in `zswap_state`.
    ///   The replay starts from the plan's registrations, so the commit keeps
    ///   each one that the replay did not consume. A replay consumes a
    ///   registration when it claims the coin. The overwrite drops a
    ///   registration made after the plan snapshot, and brings back one
    ///   forgotten after it. So callers must not change a registration
    ///   between plan and commit, as [`Wallet`](crate::Wallet) makes sure
    ///   with its sync lock.
    /// - The pending reservation set is **merged, not overwritten**:
    ///   `clear_confirmed` drops exactly the entries whose spends this
    ///   replay observed on-chain, evaluated against the pending set as it
    ///   is *now*. Reservations added after the plan snapshot survive (their
    ///   spends cannot have been observed by a replay that started earlier).
    /// - `parameters` and `block_context` are refreshed from the chain view
    ///   the replay fetched.
    ///
    /// A resync that crossed a hard fork replaces the state with the later
    /// generation's and saves it. The shielded state crosses with its
    /// registrations, and the chain pin stays as the commit finds it. The
    /// reservations do not carry over: a transaction built for the earlier
    /// generation cannot land on the chain that left it.
    pub(crate) fn commit_resync(&mut self, commit: ResyncCommit) -> Result<(), WalletError> {
        match (&mut *self, commit.0) {
            (WalletState::Ledger8(w), ResyncCommitKind::Ledger8(c)) => w.commit_resync(c),
            (WalletState::Ledger9(w), ResyncCommitKind::Ledger9(c)) => w.commit_resync(c),
            (WalletState::Ledger8(w8), ResyncCommitKind::Crossed(mut w9)) => {
                if let Some(pin) = w8.chain_pin() {
                    w9.set_chain_pin(pin.clone());
                }
                let saved = w9.persist(true);
                *self = WalletState::Ledger9(*w9);
                info!("the wallet crossed to ledger 9");
                saved
            }
            (_, commit) => Err(WalletError::LedgerMismatch {
                expected: self.ledger_version(),
                found: match commit {
                    ResyncCommitKind::Ledger9(_) => LedgerVersion::V9,
                    ResyncCommitKind::Ledger8(_) | ResyncCommitKind::Crossed(_) => {
                        LedgerVersion::V8
                    }
                },
            }),
        }
    }

    /// Snapshot the inputs of a shielded rescan's replay phase. See
    /// [`ShieldedRescanPlan`] for the intended plan → run → commit flow.
    pub(crate) fn shielded_rescan_plan(&self) -> ShieldedRescanPlan {
        let (plan, coins) = match self {
            WalletState::Ledger8(w) => (
                RescanPlanKind::Ledger8(w.shielded_rescan_plan()),
                w.rescan_coins(),
            ),
            WalletState::Ledger9(w) => (
                RescanPlanKind::Ledger9(w.shielded_rescan_plan()),
                w.rescan_coins(),
            ),
        };
        ShieldedRescanPlan {
            indexer_url: each!(self, w => w.indexer_url()).to_string(),
            identity: Arc::clone(self.identity()),
            coins,
            plan,
        }
    }

    /// Apply a completed shielded rescan to `self` and persist it when a
    /// storage directory is configured.
    ///
    /// The rebuilt shielded state and its cursor replace what the wallet
    /// holds. The plan seeds the replay with every registration, so the
    /// commit keeps each one that the replay did not consume, and callers
    /// must not change a registration between plan and commit, as for
    /// [`Self::commit_resync`]. Callers must keep resyncs from interleaving
    /// (see [`ShieldedRescanPlan::run`]). Everything else a resync writes
    /// (dust, unshielded, parameters, block context, pending reservations) is
    /// left alone here.
    pub(crate) fn commit_shielded_rescan(
        &mut self,
        commit: ShieldedRescanCommit,
    ) -> Result<(), WalletError> {
        match (self, commit.0) {
            (WalletState::Ledger8(w), RescanCommitKind::Ledger8(c)) => w.commit_shielded_rescan(c),
            (WalletState::Ledger9(w), RescanCommitKind::Ledger9(c)) => w.commit_shielded_rescan(c),
            (WalletState::Ledger8(_), RescanCommitKind::Ledger9(_)) => {
                Err(WalletError::LedgerMismatch {
                    expected: LedgerVersion::V8,
                    found: LedgerVersion::V9,
                })
            }
            (WalletState::Ledger9(_), RescanCommitKind::Ledger8(_)) => {
                Err(WalletError::LedgerMismatch {
                    expected: LedgerVersion::V9,
                    found: LedgerVersion::V8,
                })
            }
        }
    }
}

/// Snapshot of everything a resync's replay phase consumes, taken from a
/// `&WalletState` by [`WalletState::resync_plan`], with the indexer the
/// wallet synced from.
///
/// Exists so callers that share a wallet across tasks (notably
/// [`Wallet`](crate::Wallet)) can run the slow replay I/O while
/// reads and builds still reach the wallet: snapshot under a brief read lock,
/// [`ResyncPlan::run`] the replays with no read or write lock held, then apply
/// the validated result under a brief write lock via
/// [`WalletState::commit_resync`].
///
/// The fields are clones of the wallet's cursors and replay state; taking a
/// plan does not mutate or lock anything beyond the `&self` borrow.
#[must_use = "run the plan with ResyncPlan::run, then apply it with WalletState::commit_resync"]
pub(crate) struct ResyncPlan {
    indexer_url: String,
    kind: ResyncPlanKind,
}

enum ResyncPlanKind {
    Ledger8 {
        plan: ledger_8::ResyncPlan,
        crossing: Crossing,
    },
    Ledger9(ledger_9::ResyncPlan),
}

/// What a resync that crosses a hard fork needs beyond its plan, to build
/// the wallet again in the later generation.
struct Crossing {
    identity: Arc<Identity>,
    storage_dir: Option<PathBuf>,
    chain_pin: Option<ChainPin>,
}

impl ResyncPlan {
    /// Run the resync's replay phase: resume the three indexer subscriptions
    /// from the snapshotted cursors and fetch the latest block (chain time +
    /// ledger parameters), all without touching the wallet.
    ///
    /// Returns the validated [`ResyncCommit`] to apply with
    /// [`WalletState::commit_resync`]. On any replay or fetch error nothing was
    /// committed anywhere, so the wallet the plan was taken from is
    /// untouched.
    ///
    /// Callers that release the wallet lock between plan and commit must
    /// serialize resyncs themselves (two concurrent runs would replay from
    /// the same cursors and race their commits). [`Wallet`](crate::Wallet)
    /// holds a mutex across plan → run → commit for this.
    ///
    /// When the chain moved to a later generation since the wallet's last
    /// sync, the replay crosses the fork. It applies the shielded events up
    /// to the fork and carries the shielded state across. Then it replays the
    /// Dust stream into an empty state of the later generation. It writes
    /// nothing, and the commit saves the crossed state.
    pub(crate) async fn run(self) -> Result<ResyncCommit, WalletError> {
        let ResyncPlan { indexer_url, kind } = self;
        let indexer_url = indexer_url.as_str();
        match kind {
            ResyncPlanKind::Ledger9(plan) => Ok(ResyncCommit(ResyncCommitKind::Ledger9(
                plan.run(indexer_url).await?,
            ))),
            ResyncPlanKind::Ledger8 { plan, crossing } => {
                let block = latest_block(indexer_url).await?;
                let block = if ledger_of(&block)? == LedgerVersion::V8 {
                    // The chain can fork before this replay ends, and the
                    // replay then meets the later generation's events.
                    match plan.clone().run(indexer_url).await {
                        Err(WalletError::LedgerMismatch { expected, found })
                            if found > expected =>
                        {
                            info!(%found, "the chain crossed a hard fork during the resync");
                            latest_block(indexer_url).await?
                        }
                        result => return Ok(ResyncCommit(ResyncCommitKind::Ledger8(result?))),
                    }
                } else {
                    block
                };
                cross(plan, crossing, &block, indexer_url).await
            }
        }
    }
}

/// Replay a ledger 8 wallet's plan onto a chain that crossed to ledger 9,
/// and return the wallet again in ledger 9.
async fn cross(
    plan: ledger_8::ResyncPlan,
    crossing: Crossing,
    block: &midnight_indexer_client::Block,
    indexer_url: &str,
) -> Result<ResyncCommit, WalletError> {
    let ledger_8::ResyncPlan {
        dust_wallet,
        dust_event_id,
        zswap_state,
        zswap_event_id,
        unshielded_utxos,
        last_tx_id,
        last_block_height,
        ..
    } = plan;
    let from = Resume {
        zswap: ZswapState::Ledger8(zswap_state),
        zswap_event_id,
        dust: DustState::Ledger8(dust_wallet),
        dust_event_id,
        unshielded_utxos,
        last_tx_id,
        last_block_height,
    };
    let state = sync_on_chain(
        ChainSync {
            indexer_url,
            identity: &crossing.identity,
            storage_dir: crossing.storage_dir.as_deref(),
            save: false,
            chain_pin: crossing.chain_pin,
        },
        block,
        Some(from),
        None,
    )
    .await?;
    match state {
        WalletState::Ledger9(wallet) => {
            Ok(ResyncCommit(ResyncCommitKind::Crossed(Box::new(wallet))))
        }
        WalletState::Ledger8(_) => Err(WalletError::LedgerMismatch {
            expected: LedgerVersion::V9,
            found: LedgerVersion::V8,
        }),
    }
}

/// Validated results of a resync's replay tasks and latest-block fetch,
/// ready to be committed into a [`WalletState`] via
/// [`WalletState::commit_resync`].
///
/// Produced only by [`ResyncPlan::run`]; the fields are private so a commit
/// can't be forged from un-validated data. Grouping the commit inputs also
/// makes the commit-and-persist sequence unit-testable without a live
/// indexer.
#[must_use = "apply with WalletState::commit_resync, or the completed replay is discarded"]
pub(crate) struct ResyncCommit(ResyncCommitKind);

enum ResyncCommitKind {
    Ledger8(ledger_8::ResyncCommit),
    Ledger9(ledger_9::ResyncCommit),
    /// A ledger 8 wallet's resync that crossed the fork to ledger 9: the
    /// wallet again, in ledger 9.
    Crossed(Box<ledger_9::Wallet>),
}

/// Snapshot of what a shielded rescan's replay consumes, taken from a
/// `&WalletState` by [`WalletState::shielded_rescan_plan`], with the indexer
/// the wallet synced from.
///
/// A rescan replays `zswapLedgerEvents` from its first event against a state
/// that starts empty and carries the wallet's registered coins (see
/// [`WalletState::watch_for_coins`]). Starting over is the point: a replay that
/// meets an output it cannot claim collapses that Merkle leaf, and a resync
/// resumes from the cursor rather than revisiting it, so a registration made
/// after the fact is only honoured by a replay that starts at zero.
///
/// The plan / run / commit split mirrors [`ResyncPlan`], and for the same
/// reason: a caller that shares the wallet across tasks snapshots under a
/// brief read lock, runs the replay with no read or write lock held, and
/// applies the result under a brief write lock.
#[must_use = "run the plan with ShieldedRescanPlan::run, then apply it with WalletState::commit_shielded_rescan"]
pub(crate) struct ShieldedRescanPlan {
    indexer_url: String,
    identity: Arc<Identity>,
    coins: Vec<CoinInfo>,
    plan: RescanPlanKind,
}

enum RescanPlanKind {
    Ledger8(ledger_8::ShieldedRescanPlan),
    Ledger9(ledger_9::ShieldedRescanPlan),
}

impl ShieldedRescanPlan {
    /// Run the rescan's replay phase: replay the whole shielded event stream
    /// from its first event, without touching the wallet.
    ///
    /// Returns the [`ShieldedRescanCommit`] to apply with
    /// [`WalletState::commit_shielded_rescan`]. On a replay error nothing was
    /// committed anywhere, so the wallet the plan was taken from is
    /// untouched.
    ///
    /// Callers that release the wallet lock between plan and commit must
    /// serialize this against resyncs (a resync committing its own cursor and
    /// state in the middle would overwrite the rebuilt state).
    /// [`Wallet`](crate::Wallet) holds a mutex across plan → run → commit for
    /// this.
    ///
    /// A stream that starts in an earlier generation than the wallet's is
    /// replayed from that generation, and the state crosses each fork on the
    /// way.
    pub(crate) async fn run(self) -> Result<ShieldedRescanCommit, WalletError> {
        let ShieldedRescanPlan {
            indexer_url,
            identity,
            coins,
            plan,
        } = self;
        let indexer_url = indexer_url.as_str();
        let result = match plan {
            RescanPlanKind::Ledger8(plan) => {
                plan.run(indexer_url).await.map(RescanCommitKind::Ledger8)
            }
            RescanPlanKind::Ledger9(plan) => {
                plan.run(indexer_url).await.map(RescanCommitKind::Ledger9)
            }
        };
        let kind = match result {
            Err(WalletError::LedgerMismatch { expected, found }) if found < expected => {
                info!(%found, "the shielded stream starts in an earlier ledger; rescanning across the fork");
                let seed = identity.seed();
                let start = ledger_8::ShieldedRescanPlan::new(seed, &coins).initial_state;
                let sub_client = SubscriptionClient::new(indexer_url);
                let replayed = replay_zswap(
                    &sub_client,
                    seed,
                    ZswapState::Ledger8(start),
                    0,
                    false,
                    None,
                )
                .await?;
                match replayed.state.into_ledger(expected)? {
                    ZswapState::Ledger8(zswap_state) => {
                        RescanCommitKind::Ledger8(ledger_8::ShieldedRescanCommit {
                            zswap_state,
                            zswap_event_id: replayed.last_id,
                        })
                    }
                    ZswapState::Ledger9(zswap_state) => {
                        RescanCommitKind::Ledger9(ledger_9::ShieldedRescanCommit {
                            zswap_state,
                            zswap_event_id: replayed.last_id,
                        })
                    }
                }
            }
            result => result?,
        };
        Ok(ShieldedRescanCommit(kind))
    }
}

/// Validated result of a shielded rescan's replay, ready to be committed into
/// a [`WalletState`] via [`WalletState::commit_shielded_rescan`].
///
/// Produced only by [`ShieldedRescanPlan::run`]; the fields are private so a
/// commit can't be forged from un-replayed state.
#[must_use = "apply with WalletState::commit_shielded_rescan, or the completed replay is discarded"]
pub(crate) struct ShieldedRescanCommit(RescanCommitKind);

enum RescanCommitKind {
    Ledger8(ledger_8::ShieldedRescanCommit),
    Ledger9(ledger_9::ShieldedRescanCommit),
}

// ---------------------------------------------------------------------------
// The sync that crosses hard forks.
// ---------------------------------------------------------------------------

/// A shielded state, in its generation.
pub(crate) enum ZswapState {
    Ledger8(midnight_helpers::ledger_8::WalletState<DefaultDB>),
    Ledger9(midnight_helpers::ledger_9::WalletState<DefaultDB>),
}

impl ZswapState {
    fn ledger_version(&self) -> LedgerVersion {
        match self {
            Self::Ledger8(_) => LedgerVersion::V8,
            Self::Ledger9(_) => LedgerVersion::V9,
        }
    }

    /// This state in the generation `target`, carried forward across the
    /// forks in between. Both generations encode the shielded state alike,
    /// and the fork kept the chain's, so the coins and the Merkle tree carry
    /// over unchanged.
    fn into_ledger(self, target: LedgerVersion) -> Result<Self, WalletError> {
        match (self, target) {
            (Self::Ledger8(state), LedgerVersion::V9) => {
                let state = midnight_helpers::fork::old_to_new_ser(&state).map_err(|e| {
                    WalletError::Sync(format!("carry the shielded state across the fork: {e}"))
                })?;
                Ok(Self::Ledger9(state))
            }
            (state, target) if state.ledger_version() == target => Ok(state),
            (state, target) => Err(WalletError::LedgerMismatch {
                expected: target,
                found: state.ledger_version(),
            }),
        }
    }
}

/// A Dust state, in its generation. A sync resumes it only on a chain of the
/// same generation: a hard fork empties the chain's Dust state.
enum DustState {
    Ledger8(midnight_helpers::ledger_8::DustWallet<DefaultDB>),
    Ledger9(midnight_helpers::ledger_9::DustWallet<DefaultDB>),
}

/// The shielded state a replay reached, and the last event it applied.
struct ZswapReplayed {
    state: ZswapState,
    last_id: i64,
}

/// Replay the zswap stream from `start_id`, starting in `state`'s
/// generation, and cross each hard fork the stream meets.
async fn replay_zswap(
    sub_client: &SubscriptionClient,
    seed: &Seed,
    state: ZswapState,
    start_id: i64,
    resuming: bool,
    progress: Option<mpsc::Sender<SyncProgress>>,
) -> Result<ZswapReplayed, WalletError> {
    let (mut state, mut start_id) = (state, start_id);
    loop {
        let (replayed, later) = match state {
            ZswapState::Ledger8(zswap) => {
                let keys = ledger_8::state::zswap_keys(seed);
                let replay = ledger_8::state::replay_zswap_events(
                    sub_client,
                    &keys,
                    zswap,
                    start_id,
                    resuming,
                    progress.clone(),
                )
                .await?;
                (
                    ZswapReplayed {
                        state: ZswapState::Ledger8(replay.state),
                        last_id: replay.last_id,
                    },
                    replay.later_ledger,
                )
            }
            ZswapState::Ledger9(zswap) => {
                let keys = ledger_9::state::zswap_keys(seed);
                let replay = ledger_9::state::replay_zswap_events(
                    sub_client,
                    &keys,
                    zswap,
                    start_id,
                    resuming,
                    progress.clone(),
                )
                .await?;
                (
                    ZswapReplayed {
                        state: ZswapState::Ledger9(replay.state),
                        last_id: replay.last_id,
                    },
                    replay.later_ledger,
                )
            }
        };
        let Some((at, found)) = later else {
            return Ok(replayed);
        };
        info!(at, %found, "the shielded stream crosses a hard fork");
        state = replayed.state.into_ledger(found)?;
        start_id = at;
    }
}

/// Where a sync resumes: a snapshot, or a wallet in memory.
struct Resume {
    zswap: ZswapState,
    /// The last shielded event the state applied.
    zswap_event_id: i64,
    dust: DustState,
    /// The last Dust event the state consumed.
    dust_event_id: i64,
    unshielded_utxos: Vec<TrackedUtxo>,
    last_tx_id: Option<i64>,
    last_block_height: i64,
}

/// Who a sync is for, and where it keeps what it learns.
#[derive(Clone)]
pub(crate) struct ChainSync<'a> {
    pub(crate) indexer_url: &'a str,
    pub(crate) identity: &'a Arc<Identity>,
    pub(crate) storage_dir: Option<&'a Path>,
    /// Whether the sync writes its state under `storage_dir` as it goes:
    /// Dust checkpoints, the pending reservations it loads, and the synced
    /// state. A sync whose state waits for a commit writes nothing, and the
    /// commit saves it.
    pub(crate) save: bool,
    /// The pin this sync persists, so a later resume can ask whether it is
    /// still on this chain.
    pub(crate) chain_pin: Option<ChainPin>,
}

/// One sync attempt: read the chain's generation, load the snapshot in its
/// own generation, and sync on the chain.
async fn sync_once(
    sync: ChainSync<'_>,
    progress: Option<mpsc::Sender<SyncProgress>>,
) -> Result<WalletState, WalletError> {
    let block = latest_block(sync.indexer_url).await?;
    let chain = ledger_of(&block)?;
    let wallet_id = wallet_storage_id(sync.identity.unshielded_address());

    info!("loading cached state from disk");
    let snapshot = match sync.storage_dir {
        Some(dir) => load_snapshot(dir, sync.identity.network().as_str(), &wallet_id, chain)?,
        None => None,
    };
    let (resume, sync) = match snapshot {
        Some((resume, stored_pin)) => {
            info!(
                zswap_event_id = resume.zswap_event_id,
                dust_event_id = resume.dust_event_id,
                ledger = %resume.zswap.ledger_version(),
                "resuming from cached state"
            );
            let alive = send_progress(
                &progress,
                SyncProgress::Resuming {
                    zswap_event_id: resume.zswap_event_id,
                    dust_event_id: resume.dust_event_id,
                },
            );
            if !alive {
                return Err(progress_cancelled("resume"));
            }
            // Keep the snapshot's own pin when this sync has no fresher one.
            // A node that could not answer must not cost the wallet the mark
            // that lets the next resume check itself.
            let chain_pin = sync.chain_pin.or(stored_pin);
            (Some(resume), ChainSync { chain_pin, ..sync })
        }
        None => (None, sync),
    };
    sync_on_chain(sync, &block, resume, progress).await
}

/// A snapshot, in the generation that wrote it, and the chain pin it
/// carries. A snapshot of a later generation than the chain's means the
/// chain was replaced.
fn load_snapshot(
    dir: &Path,
    network_id: &str,
    wallet_id: &str,
    chain: LedgerVersion,
) -> Result<Option<(Resume, Option<ChainPin>)>, WalletError> {
    let Some(stored) = crate::storage::load_ledger_version(dir, network_id, wallet_id)? else {
        return Ok(None);
    };
    if stored > chain {
        return Err(WalletError::LedgerRegression {
            path: crate::storage::snapshot_path(dir, network_id, wallet_id)
                .display()
                .to_string(),
            snapshot: stored,
            chain,
        });
    }
    Ok(match stored {
        LedgerVersion::V8 => ledger_8::snapshot::load(dir, network_id, wallet_id)?.map(|c| {
            let resume = Resume {
                zswap: ZswapState::Ledger8(c.zswap_state),
                zswap_event_id: c.zswap_event_id,
                dust: DustState::Ledger8(c.dust_wallet),
                dust_event_id: c.dust_event_id,
                unshielded_utxos: c.unshielded_utxos,
                last_tx_id: c.last_tx_id,
                last_block_height: c.last_block_height,
            };
            (resume, c.chain_pin)
        }),
        LedgerVersion::V9 => ledger_9::snapshot::load(dir, network_id, wallet_id)?.map(|c| {
            let resume = Resume {
                zswap: ZswapState::Ledger9(c.zswap_state),
                zswap_event_id: c.zswap_event_id,
                dust: DustState::Ledger9(c.dust_wallet),
                dust_event_id: c.dust_event_id,
                unshielded_utxos: c.unshielded_utxos,
                last_tx_id: c.last_tx_id,
                last_block_height: c.last_block_height,
            };
            (resume, c.chain_pin)
        }),
    })
}

/// Sync on a chain whose latest block is `block`, from `resume` or from
/// genesis. Replay the shielded and unshielded streams, and cross each fork
/// the shielded stream meets. Then finish in the chain's generation.
async fn sync_on_chain(
    sync: ChainSync<'_>,
    block: &midnight_indexer_client::Block,
    resume: Option<Resume>,
    progress: Option<mpsc::Sender<SyncProgress>>,
) -> Result<WalletState, WalletError> {
    let chain = ledger_of(block)?;
    let resuming = resume.is_some();
    // Where each stream starts. When resuming, start from the next event
    // after the last one applied; the subscription is inclusive, so the
    // last one itself would be delivered again. A replay from genesis
    // starts in the earliest generation this build links, and crosses
    // forward to wherever the chain's history leads.
    let (zswap_start, start_zswap_id, dust_state, start_dust_id, utxos, start_tx_id, height) =
        match resume {
            Some(r) => (
                r.zswap,
                r.zswap_event_id + 1,
                Some(r.dust),
                r.dust_event_id + 1,
                r.unshielded_utxos,
                r.last_tx_id.map(|id| id + 1).unwrap_or(0),
                r.last_block_height,
            ),
            None => (
                ZswapState::Ledger8(midnight_helpers::ledger_8::WalletState::new()),
                0,
                None,
                0,
                Vec::new(),
                0,
                0,
            ),
        };

    // The fork from ledger 8 to ledger 9 emptied the chain's Dust state and
    // its registrations, so no UTXO the wallet held before the fork generates
    // Dust. The replay's events carry the indexer's current flags.
    let mut utxos = utxos;
    if matches!(dust_state, Some(DustState::Ledger8(_))) && chain == LedgerVersion::V9 {
        for utxo in &mut utxos {
            utxo.registered_for_dust_generation = Some(false);
        }
    }

    let sub_client = SubscriptionClient::new(sync.indexer_url);
    info!(
        start_zswap_id,
        start_tx_id, start_dust_id, "starting subscriptions"
    );
    let (zswap, unshielded) = tokio::join!(
        replay_zswap(
            &sub_client,
            sync.identity.seed(),
            zswap_start,
            start_zswap_id,
            resuming,
            progress.clone(),
        ),
        replay_unshielded_events(
            &sub_client,
            sync.identity.unshielded_address(),
            utxos,
            start_tx_id,
            height,
            resuming,
            progress.clone(),
        ),
    );
    let zswap = zswap?;
    let UnshieldedReplay {
        utxos: unshielded_utxos,
        last_tx_id,
        last_block_height,
        spent: spent_unshielded,
    } = unshielded?;

    match zswap.state.into_ledger(chain)? {
        ZswapState::Ledger8(zswap_state) => {
            let dust = match dust_state {
                Some(DustState::Ledger8(wallet)) => ledger_8::state::DustStart::Resume {
                    wallet: Box::new(wallet),
                    next_id: start_dust_id,
                },
                _ => ledger_8::state::DustStart::Fresh {
                    next_id: start_dust_id,
                },
            };
            let wallet = ledger_8::Wallet::finish_sync(
                sync,
                block,
                ledger_8::state::SyncStart {
                    zswap_state,
                    zswap_event_id: zswap.last_id,
                    unshielded_utxos,
                    last_tx_id,
                    last_block_height,
                    spent_unshielded,
                    dust,
                },
                progress,
            )
            .await?;
            Ok(WalletState::Ledger8(wallet))
        }
        ZswapState::Ledger9(zswap_state) => {
            let dust = match dust_state {
                Some(DustState::Ledger9(wallet)) => ledger_9::state::DustStart::Resume {
                    wallet: Box::new(wallet),
                    next_id: start_dust_id,
                },
                Some(DustState::Ledger8(_)) => {
                    warn!(
                        "the chain crossed a hard fork that emptied its Dust state; Dust starts over"
                    );
                    ledger_9::state::DustStart::Fresh {
                        next_id: start_dust_id,
                    }
                }
                None => ledger_9::state::DustStart::Fresh {
                    next_id: start_dust_id,
                },
            };
            let wallet = ledger_9::Wallet::finish_sync(
                sync,
                block,
                ledger_9::state::SyncStart {
                    zswap_state,
                    zswap_event_id: zswap.last_id,
                    unshielded_utxos,
                    last_tx_id,
                    last_block_height,
                    spent_unshielded,
                    dust,
                },
                progress,
            )
            .await?;
            Ok(WalletState::Ledger9(wallet))
        }
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use midnight_types::hd::Seed;
    use midnight_types::{CoinInfo, HashOutput, Nonce, ShieldedTokenType};

    pub(crate) fn test_seed() -> Seed {
        Seed::from_bytes(&[0x22; 32]).unwrap()
    }

    /// A coin the wallet owns and can rebuild, but whose output carries
    /// nothing it can decrypt.
    pub(crate) fn unreachable_coin(byte: u8, value: u128) -> CoinInfo {
        CoinInfo {
            nonce: Nonce(HashOutput([byte; 32])),
            type_: ShieldedTokenType(HashOutput([3u8; 32])),
            value,
        }
    }
}

#[cfg(test)]
mod tests {
    use midnight_helpers::{Tagged, ledger_8, ledger_9};
    use midnight_indexer_client::testutil::{
        accept_subscriber, bind, ledger_event, next_json, send_next, subscription_variable,
    };

    use super::test_support::{test_seed, unreachable_coin};
    use super::*;

    /// A chain that crossed the fork serves ledger 8 events, then ledger 9
    /// ones, in one stream. A replay that stopped at the boundary, or lost the
    /// state it built before it, would lose the coins of one half. The second
    /// coin is claimed by a registration made before the fork, which the
    /// crossing must carry.
    #[tokio::test]
    async fn a_zswap_replay_crosses_the_fork_with_the_coins_of_both_halves() {
        let (listener, url) = bind().await;
        let events = [
            (
                1,
                crate::ledger_8::state::test_support::ciphertextless_output(
                    &test_seed(),
                    &unreachable_coin(1, 42),
                    0,
                ),
            ),
            (
                2,
                crate::ledger_9::state::test_support::ciphertextless_output(
                    &test_seed(),
                    &unreachable_coin(2, 42),
                    1,
                ),
            ),
        ];
        let server = tokio::spawn(async move {
            // One subscription per generation the replay passes through.
            for _ in 0..2 {
                let (mut ws, sub) = accept_subscriber(&listener).await;
                let from = subscription_variable(&sub, "id");
                for (id, raw) in events.iter().filter(|(id, _)| *id >= from) {
                    let message = ledger_event("zswapLedgerEvents", *id, raw, 2);
                    send_next(&mut ws, &sub, message).await;
                }
                while next_json(&mut ws).await.is_some() {}
            }
        });

        let registered = crate::ledger_8::state::ShieldedRescanPlan::new(
            &test_seed(),
            &[unreachable_coin(1, 42), unreachable_coin(2, 42)],
        );
        let replayed = replay_zswap(
            &SubscriptionClient::new(&url),
            &test_seed(),
            ZswapState::Ledger8(registered.initial_state),
            0,
            false,
            None,
        )
        .await
        .expect("the replay crosses the fork");
        server.await.unwrap();

        assert_eq!(replayed.last_id, 2);
        let ZswapState::Ledger9(state) = replayed.state else {
            panic!("the state must end in the generation of the last event");
        };
        assert_eq!(state.coins.iter().count(), 2, "a coin from each half");
        assert_eq!(
            state.pending_outputs.iter().count(),
            0,
            "both registrations are claimed"
        );
    }

    /// The fork emptied the chain's Dust state, and the indexer still serves
    /// the Dust events from before it. A ledger 9 replay that decoded them
    /// would fail, and one that applied them would hold Dust that no longer
    /// exists.
    #[tokio::test]
    async fn a_ledger_9_dust_replay_skips_ledger_8_events_undecoded() {
        let (listener, url) = bind().await;
        // Only the tag says ledger 8; the body would not decode.
        let raw = hex::encode(format!(
            "midnight:{}:garbage",
            ledger_8::Event::<DefaultDB>::tag()
        ));
        let server = tokio::spawn(async move {
            let (mut ws, sub) = accept_subscriber(&listener).await;
            for id in 1..=3 {
                let message = ledger_event("dustLedgerEvents", id, &raw, 3);
                send_next(&mut ws, &sub, message).await;
            }
            while next_json(&mut ws).await.is_some() {}
        });

        let wallet = ledger_9::DustWallet::default(
            midnight_types::ledger_9::convert::IntoLedger::into_ledger(&test_seed()),
            Some(&ledger_9::INITIAL_PARAMETERS),
        );
        let replay = crate::ledger_9::state::replay_dust_events(
            &SubscriptionClient::new(&url),
            wallet,
            0,
            false,
            None::<fn(&ledger_9::DustWallet<DefaultDB>, i64)>,
            None,
        )
        .await
        .expect("a ledger 9 replay skips ledger 8 events");
        server.await.unwrap();

        assert_eq!(
            replay.last_id, 3,
            "the skipped events still move the cursor"
        );
        assert!(replay.later_ledger.is_none());
        assert!(replay.spend_nullifiers.is_empty());
    }

    /// Pointing a stored ledger 9 wallet at a ledger 8 chain, such as a
    /// devnet recreated on the other generation, must say which directory to
    /// remove, as the chain pin's error does.
    #[test]
    fn a_snapshot_of_a_later_generation_names_the_directory_to_remove() {
        let base = tempfile::TempDir::new().unwrap();
        let dir = crate::storage::snapshot_path(base.path(), "undeployed", "wallet");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("metadata.json"),
            r#"{
                "generation": 1,
                "ledger_version": "V9",
                "zswap_event_id": 1,
                "dust_event_id": 1,
                "last_block_height": 0,
                "last_tx_id": null,
                "unshielded_utxos": []
            }"#,
        )
        .unwrap();

        let Err(err) = load_snapshot(base.path(), "undeployed", "wallet", LedgerVersion::V8) else {
            panic!("a ledger 9 snapshot must not load for a ledger 8 chain");
        };
        let message = err.to_string();
        assert!(
            matches!(err, WalletError::LedgerRegression { .. }),
            "{message}"
        );
        assert!(
            message.contains(&dir.display().to_string()),
            "the error must name what to remove: {message}"
        );
    }
}
