//! A synced wallet on whichever ledger generation its chain runs, and the
//! sync that carries it across a hard fork.

use std::path::{Path, PathBuf};

use midnight_helpers::DefaultDB;
use midnight_indexer_client::SubscriptionClient;
use midnight_types::chain_pin::ChainPin;
use midnight_types::{
    ChainParameters, CoinInfo, CoinPublicKey, DustBalance, EncryptionPublicKey, LedgerVersion,
    Network, ShieldedBalance, SpendableShieldedCoin, SpentInputs, SyncCursors, TrackedUtxo,
    UnshieldedUtxoInfo, WalletBalance, WalletError, WalletSeed,
};
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::replay::{latest_block, progress_cancelled, replay_unshielded_events, send_progress};
use crate::storage::wallet_storage_id;
use crate::{SyncProgress, ledger_8, ledger_9};

/// A Midnight wallet: identity (seed, addresses) and synced ledger state.
///
/// Maintains three streams of state from the indexer:
/// - `zswapLedgerEvents` → shielded coin tracking + Merkle tree
/// - `dustLedgerEvents` → dust/fee UTXO tracking
/// - `unshieldedTransactions` → unshielded UTXO balance
///
/// The state is in the ledger generation its chain runs
/// ([`Self::ledger_version`]). A sync reads the generation from the chain.
/// A sync or resync that meets a hard fork carries the wallet across it. The
/// shielded state continues in the new generation, and the Dust state starts
/// empty, because the fork emptied the chain's Dust state.
///
/// All I/O is driven by `midnight_provider::MidnightProvider`, which reaches
/// the wallet through `WalletFacade`.
pub struct Wallet {
    seed: WalletSeed,
    state: State,
}

/// The wallet's state, in its generation.
pub(crate) enum State {
    Ledger8(ledger_8::Wallet),
    Ledger9(ledger_9::Wallet),
}

/// Evaluate `$body` with `$w` bound to the state of whichever generation
/// `$state` holds.
macro_rules! each {
    ($state:expr, $w:ident => $body:expr) => {
        match $state {
            State::Ledger8($w) => $body,
            State::Ledger9($w) => $body,
        }
    };
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

fn seed_8(seed: &WalletSeed) -> midnight_helpers::ledger_8::WalletSeed {
    midnight_types::ledger_8::convert::IntoLedger::into_ledger(seed)
}

fn seed_9(seed: &WalletSeed) -> midnight_helpers::ledger_9::WalletSeed {
    midnight_types::ledger_9::convert::IntoLedger::into_ledger(seed)
}

impl Wallet {
    pub(crate) fn state(&self) -> &State {
        &self.state
    }

    pub(crate) fn state_mut(&mut self) -> &mut State {
        &mut self.state
    }

    /// Default storage directory: `~/.midnight/wallets/`
    pub fn default_storage_dir() -> Option<PathBuf> {
        home_dir().map(|h| h.join(".midnight").join("wallets"))
    }

    /// The ledger generation this wallet's state is in.
    pub fn ledger_version(&self) -> LedgerVersion {
        match self.state {
            State::Ledger8(_) => LedgerVersion::V8,
            State::Ledger9(_) => LedgerVersion::V9,
        }
    }

    /// The chain pin a snapshot on disk carries, or `None` when there is no
    /// snapshot or it predates the pin.
    ///
    /// Read this before a resume and judge it with
    /// [`crate::chain_pin::verify_pin`], against what the node reports for
    /// that height. A snapshot from a chain that no longer exists still
    /// resumes cleanly and reports the old balance, so nothing later in the
    /// sync will catch it.
    pub fn stored_chain_pin(
        storage_dir: &Path,
        network: impl Into<Network>,
        address: &str,
    ) -> Result<Option<ChainPin>, WalletError> {
        let network = network.into();
        crate::storage::load_chain_pin(storage_dir, network.as_str(), &wallet_storage_id(address))
    }

    /// Where the snapshot for `address` lives, so an error can name what to
    /// remove.
    pub fn snapshot_path(
        storage_dir: &Path,
        network: impl Into<Network>,
        address: &str,
    ) -> PathBuf {
        let network = network.into();
        crate::storage::snapshot_path(storage_dir, network.as_str(), &wallet_storage_id(address))
    }

    /// The sync that [`Wallet::sync`] runs, for both `.await` and `.stream()`.
    ///
    /// Reads the generation the chain runs from its latest block, resumes
    /// from a snapshot under `storage_dir` when there is one, and replays the
    /// three event streams, crossing each hard fork they meet. Checkpoints
    /// Dust progress to disk periodically, so an interrupted sync resumes
    /// where it left off.
    pub(crate) async fn sync_inner(
        indexer_url: &str,
        seed: WalletSeed,
        address: &str,
        network: impl Into<Network>,
        storage_dir: Option<&Path>,
        // The finalized block the node reports now, persisted so a later
        // resume can ask whether it is still on this chain. `None` skips the
        // pin, which is what a caller without node access must pass.
        chain_pin: Option<ChainPin>,
        progress: Option<mpsc::Sender<SyncProgress>>,
    ) -> Result<Self, WalletError> {
        let network = network.into();
        let sync = |progress| {
            sync_once(
                indexer_url,
                &seed,
                address,
                network.as_str(),
                storage_dir,
                chain_pin.clone(),
                progress,
            )
        };
        // A chain can fork between the read of its latest block and the
        // replay of its events. The replay then meets the later generation,
        // and a second attempt reads the block again and starts from there.
        let state = match sync(progress.clone()).await {
            Err(WalletError::LedgerMismatch { expected, found }) if found > expected => {
                info!(%found, "the chain moved to a later ledger during the sync; syncing again");
                sync(progress).await?
            }
            result => result?,
        };
        Ok(Self { seed, state })
    }

    /// Whether the dust state has been synced (required for transaction
    /// building).
    pub fn dust_synced(&self) -> bool {
        each!(&self.state, w => w.dust_synced())
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
    ///
    /// Persistence is best-effort, since the in-memory release already frees
    /// the inputs for this process.
    pub fn release(&mut self, spent: &SpentInputs) {
        each!(&mut self.state, w => w.release(spent))
    }

    /// Whether the confirmed state shows every input that `spent` names as
    /// spent. See [`WalletFacade::has_observed`](crate::WalletFacade::has_observed).
    pub fn has_observed(&self, spent: &[SpentInputs]) -> bool {
        each!(&self.state, w => w.has_observed(spent))
    }

    /// Save the current wallet state to disk.
    ///
    /// Writes the confirmed-state files (`metadata.json`, `zswap-N.bin`,
    /// `dust_wallet-N.bin`) and the in-flight reservations to a separate
    /// `pending.json`. Confirmed and pending live in distinct files so a
    /// failed save of one does not corrupt the other. Runs automatically at
    /// the end of initial sync and after every successful [`Wallet::resync`]
    /// when a storage directory is configured; calling it manually is only
    /// needed for extra checkpoints.
    pub fn save(&self, base: &Path) -> Result<(), WalletError> {
        each!(&self.state, w => w.save(base))
    }

    /// Where this wallet's snapshot lives, when it persists one.
    ///
    /// An error that tells a reader to remove the snapshot has to name it, so
    /// this is the path that goes in the message.
    pub fn snapshot_dir(&self) -> Option<PathBuf> {
        each!(&self.state, w => w.snapshot_dir())
    }

    /// The finalized block this wallet is pinned to, held in memory.
    ///
    /// A resume checks the pin on disk; a wallet that stays attached has to
    /// check this one, because its cursors go just as stale when the chain is
    /// replaced underneath it.
    pub fn chain_pin(&self) -> Option<&ChainPin> {
        each!(&self.state, w => w.chain_pin())
    }

    /// Move the pin to the block a fresh check saw, so it stays inside an
    /// archive's retention window rather than ageing out of it.
    pub fn set_chain_pin(&mut self, pin: ChainPin) {
        each!(&mut self.state, w => w.set_chain_pin(pin))
    }

    /// The indexer this wallet synced from, and the only one its cursors
    /// mean anything against.
    pub fn indexer_url(&self) -> &str {
        each!(&self.state, w => w.indexer_url())
    }

    /// Height of the latest block seen in an unshielded transaction event.
    ///
    /// This is NOT a general chain-sync cursor. It only advances when the
    /// wallet's unshielded address appears in a transaction.
    pub fn last_block_height(&self) -> i64 {
        each!(&self.state, w => w.last_block_height())
    }

    /// Indexer id of the latest transaction the wallet applied.
    pub fn last_tx_id(&self) -> Option<i64> {
        each!(&self.state, w => w.last_tx_id())
    }

    /// Highest zswap event id the wallet applied.
    pub fn zswap_event_id(&self) -> i64 {
        each!(&self.state, w => w.zswap_event_id())
    }

    /// Highest dust event id the wallet applied.
    pub fn dust_event_id(&self) -> i64 {
        each!(&self.state, w => w.dust_event_id())
    }

    /// How far the sync has reached, as one snapshot.
    ///
    /// The four cursors advance together during a sync, so reading them one
    /// at a time can report a mixture of two syncs. Take them here instead.
    pub fn sync_cursors(&self) -> SyncCursors {
        each!(&self.state, w => w.sync_cursors())
    }

    /// The seed this wallet signs and derives with.
    pub fn seed(&self) -> &WalletSeed {
        &self.seed
    }

    /// The wallet's shielded public keys: the coin public key an output
    /// commits to, and the encryption key its discovery ciphertext is sealed
    /// to.
    ///
    /// Public material, so anything that only needs to address a coin to this
    /// wallet can take these instead of the seed. A signer that never releases
    /// its seed can still supply them.
    pub fn shielded_public_keys(&self) -> (CoinPublicKey, EncryptionPublicKey) {
        each!(&self.state, w => w.shielded_public_keys())
    }

    /// The network identifier this wallet derives addresses for
    /// (e.g. `"undeployed"`, `"testnet"`). Returned as `&str` because the
    /// wallet stores the literal name from the bech32 HRP; callers that want
    /// the typed form can use `Network::from(wallet.network())`.
    pub fn network(&self) -> &str {
        each!(&self.state, w => w.network())
    }

    /// The wallet's unshielded receiving address.
    pub fn unshielded_address(&self) -> String {
        each!(&self.state, w => w.unshielded_address())
    }

    /// The wallet's shielded receiving address, e.g. `mn_shield-addr_undeployed1...`.
    pub fn shielded_address(&self) -> String {
        each!(&self.state, w => w.shielded_address())
    }

    /// The unshielded UTXOs this wallet tracks.
    pub fn unshielded_utxos(&self) -> &[TrackedUtxo] {
        each!(&self.state, w => w.unshielded_utxos())
    }

    /// The chain's Dust and TTL parameters, as this wallet last synced them.
    pub fn parameters(&self) -> ChainParameters {
        each!(&self.state, w => w.chain_parameters())
    }

    pub fn balance(&self) -> WalletBalance {
        each!(&self.state, w => w.balance())
    }

    pub fn dust_balance(&self) -> DustBalance {
        each!(&self.state, w => w.dust_balance())
    }

    pub fn unshielded_balance(&self) -> Vec<UnshieldedUtxoInfo> {
        each!(&self.state, w => w.unshielded_balance())
    }

    pub fn shielded_balance(&self) -> ShieldedBalance {
        each!(&self.state, w => w.shielded_balance())
    }

    /// Enumerate the wallet's spendable shielded coins, each with its full
    /// coin info (nonce, token type, value) plus the nullifier that pins it.
    ///
    /// Use this to address a specific coin for a circuit that spends it (e.g.
    /// `receiveShielded`): build the `ShieldedCoinInfo` argument from the
    /// coin's `nonce`/`token_type`/`value`, then hand the same coin back to the
    /// call builder so the SDK spends that exact coin as the shielded input.
    /// Coins a still-pending build already spent are left out.
    pub fn spendable_shielded_coins(&self) -> Vec<SpendableShieldedCoin> {
        each!(&self.state, w => w.spendable_shielded_coins())
    }

    /// Register a coin this wallet owns but cannot discover, so a replay
    /// claims it without decrypting anything.
    ///
    /// A shielded coin normally reaches its owner through the discovery
    /// ciphertext on its output. That channel fails when the party that built
    /// the transaction attached no ciphertext, or sealed it to a key this
    /// wallet does not hold. The coin is still owned by this wallet's coin
    /// public key and still spendable, as long as its owner can rebuild the
    /// `CoinInfo` (nonce, token type, value) from somewhere else. A contract
    /// that evolves one public nonce per mint is such a case.
    ///
    /// Registration records the coin's commitment under this wallet's coin
    /// public key. A replay that meets the matching output claims the coin
    /// from that record.
    ///
    /// **Registration alone does not recover a coin whose output the wallet
    /// already replayed past.** A replay that meets an output it cannot claim
    /// collapses that Merkle leaf, and a resync resumes from the cursor
    /// instead of revisiting it. Follow the registration with
    /// [`Self::rescan_shielded`], which replays the stream from its first
    /// event. `MidnightProvider::watch_for_coin` does both.
    ///
    /// A registration is part of the wallet state, so it is persisted with it
    /// when a storage directory is configured, and a coin registered before
    /// it lands on chain survives a restart. The in-memory registration
    /// stands even when that save fails.
    pub fn watch_for_coin(&mut self, coin: CoinInfo) -> Result<(), WalletError> {
        each!(&mut self.state, w => w.watch_for_coin(coin))
    }

    /// [`Self::watch_for_coin`] for several coins, persisting once.
    pub fn watch_for_coins(
        &mut self,
        coins: impl IntoIterator<Item = CoinInfo>,
    ) -> Result<(), WalletError> {
        each!(&mut self.state, w => w.watch_for_coins(coins))
    }

    /// Drop a registration [`Self::watch_for_coin`] made, for a coin that
    /// turned out to be wrong.
    ///
    /// A registration whose `CoinInfo` does not match the coin an on-chain
    /// output commits to never matches anything, so it would otherwise sit in
    /// [`Self::watched_coins`] and be re-registered by every later replay.
    ///
    /// This drops the registration only. A coin the wallet already claimed is
    /// untouched, and stays spendable: it is held now, not watched for.
    /// Forgetting a coin that was never registered does nothing.
    pub fn forget_coin(&mut self, coin: CoinInfo) -> Result<(), WalletError> {
        each!(&mut self.state, w => w.forget_coin(coin))
    }

    /// [`Self::forget_coin`] for several coins, persisting once.
    pub fn forget_coins(
        &mut self,
        coins: impl IntoIterator<Item = CoinInfo>,
    ) -> Result<(), WalletError> {
        each!(&mut self.state, w => w.forget_coins(coins))
    }

    /// Coins registered with [`Self::watch_for_coin`] that no replay has
    /// claimed yet, ordered by nonce, then token type, then value.
    ///
    /// A coin leaves this set when a replay meets its output. One still here
    /// after a [`Self::rescan_shielded`] is either not on chain yet, or its
    /// rebuilt `CoinInfo` is not the coin the on-chain output commits to.
    /// Drop such a coin with [`Self::forget_coin`].
    pub fn watched_coins(&self) -> Vec<CoinInfo> {
        each!(&self.state, w => w.watched_coins())
    }

    /// Re-sync the wallet state from the indexer, resuming from current cursors.
    ///
    /// Call this after a transaction is finalized to pick up the on-chain
    /// effects (spent dust UTXOs, new coins, etc.) before building the
    /// next transaction. The indexer serves those effects a moment after
    /// finality, so one resync can miss them. `MidnightProvider::wait_observed`
    /// resyncs until the wallet sees the spends of a transaction.
    ///
    /// When the chain crossed a hard fork since the last sync or resync, the
    /// resync carries the wallet across it, as a sync does. The shielded
    /// state continues in the later generation, and the Dust state starts
    /// empty. The resync drops the reservations of the earlier generation.
    ///
    /// On a replay or fetch error, `self` is left untouched: all results are
    /// awaited and validated before any field is mutated. The chain's current
    /// block_time is fetched as part of the same operation; failure to fetch
    /// it is also fatal because `block_context.tblock` drives TTL and proof
    /// root lookup. Ledger parameters are refreshed from the same latest
    /// block, so governance changes to fees/TTL/dust rates take effect on
    /// the next build.
    ///
    /// When the wallet was synced with a storage directory, the committed
    /// state is re-persisted before returning so a crash does not lose the
    /// moved cursors or resurrect cleared reservations. Persistence is
    /// skipped when the resync changed no durable state (no cursor moved, no
    /// reservation cleared, parameters unchanged), since resyncs run before
    /// every build and a no-op must not rewrite the generation files. A
    /// persistence failure surfaces as [`WalletError::Storage`] with the
    /// in-memory state already updated.
    ///
    /// This is the single-task composition of the three-step resync API:
    /// [`Self::resync_plan`] → [`ResyncPlan::run`] → [`Self::commit_resync`].
    /// It holds `&mut self` across the replay I/O, which is fine for an
    /// exclusively-owned wallet but serializes every reader when the wallet
    /// lives behind a lock; lock-sharing callers should drive the three
    /// steps themselves and only hold the lock around the snapshot and the
    /// commit.
    pub async fn resync(&mut self, indexer_url: &str) -> Result<(), WalletError> {
        let commit = self.resync_plan().run(indexer_url).await?;
        self.commit_resync(commit)
    }

    /// Snapshot the inputs of a resync's replay phase. See [`ResyncPlan`].
    pub fn resync_plan(&self) -> ResyncPlan {
        ResyncPlan(match &self.state {
            State::Ledger8(w) => ResyncPlanKind::Ledger8 {
                plan: w.resync_plan(),
                identity: WalletIdentity {
                    seed: self.seed.clone(),
                    network_id: w.network().to_string(),
                    storage_dir: w.storage_dir().map(Path::to_path_buf),
                    chain_pin: w.chain_pin().cloned(),
                    last_block_height: w.last_block_height(),
                },
            },
            State::Ledger9(w) => ResyncPlanKind::Ledger9(w.resync_plan()),
        })
    }

    /// Apply validated resync results to `self` and persist when (and only
    /// when) durable state changed. Separate from [`ResyncPlan::run`], which
    /// performs the I/O and validation, so this sequence is unit-testable
    /// without an indexer and so lock-sharing callers can scope their write
    /// lock to this call alone.
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
    /// - Coin registrations ([`Self::watch_for_coin`]) are **carried over**.
    ///   They live in `zswap_state`, so one made after the plan snapshot
    ///   would otherwise be dropped by the overwrite. A registration this
    ///   replay claimed is not carried, since the coin is now held.
    /// - The pending reservation set is **merged, not overwritten**:
    ///   `clear_confirmed` drops exactly the entries whose spends this
    ///   replay observed on-chain, evaluated against the pending set as it
    ///   is *now*. Reservations added after the plan snapshot survive (their
    ///   spends cannot have been observed by a replay that started earlier).
    /// - `parameters` and `block_context` are refreshed from the chain view
    ///   the replay fetched.
    ///
    /// A resync that crossed a hard fork replaces the state with the later
    /// generation's and saves it. Registrations carry over as above, and the
    /// chain pin stays as the commit finds it. The reservations do not carry
    /// over: a transaction built for the earlier generation cannot land on
    /// the chain that left it.
    pub fn commit_resync(&mut self, commit: ResyncCommit) -> Result<(), WalletError> {
        match (&mut self.state, commit.0) {
            (State::Ledger8(w), ResyncCommitKind::Ledger8(c)) => w.commit_resync(c),
            (State::Ledger9(w), ResyncCommitKind::Ledger9(c)) => w.commit_resync(c),
            (State::Ledger8(w8), ResyncCommitKind::Crossed(mut w9)) => {
                w9.carry_registrations(w8.watched_coins());
                if let Some(pin) = w8.chain_pin() {
                    w9.set_chain_pin(pin.clone());
                }
                let dir = w9.storage_dir().map(Path::to_path_buf);
                self.state = State::Ledger9(*w9);
                info!("the wallet crossed to ledger 9");
                match dir {
                    Some(dir) => self.save(&dir),
                    None => Ok(()),
                }
            }
            (state, commit) => Err(WalletError::LedgerMismatch {
                expected: match state {
                    State::Ledger8(_) => LedgerVersion::V8,
                    State::Ledger9(_) => LedgerVersion::V9,
                },
                found: match commit {
                    ResyncCommitKind::Ledger9(_) => LedgerVersion::V9,
                    ResyncCommitKind::Ledger8(_) | ResyncCommitKind::Crossed(_) => {
                        LedgerVersion::V8
                    }
                },
            }),
        }
    }

    /// Replay the shielded event stream from its first event, so the coins
    /// registered with [`Self::watch_for_coin`] are claimed.
    ///
    /// Rebuilds `zswap_state` and its cursor from the chain's events; the
    /// dust and unshielded state, their cursors, and the pending reservations
    /// are untouched. Coins the wallet already held are re-registered before
    /// the replay, so nothing is lost by starting over.
    ///
    /// The replay covers the whole stream, which costs more than a resync.
    /// Call it to recover a coin, not on a schedule.
    ///
    /// On a replay error `self` is left untouched. When a storage directory
    /// is configured the rebuilt state is persisted before returning.
    ///
    /// This is the single-task composition of the three-step rescan API:
    /// [`Self::shielded_rescan_plan`] → [`ShieldedRescanPlan::run`] →
    /// [`Self::commit_shielded_rescan`]. It holds `&mut self` across the
    /// replay I/O; callers that share the wallet behind a lock should drive
    /// the three steps themselves, as `MidnightProvider::rescan_shielded`
    /// does.
    pub async fn rescan_shielded(&mut self, indexer_url: &str) -> Result<(), WalletError> {
        let commit = self.shielded_rescan_plan().run(indexer_url).await?;
        self.commit_shielded_rescan(commit)
    }

    /// Snapshot the inputs of a shielded rescan's replay phase. See
    /// [`ShieldedRescanPlan`] for the intended plan → run → commit flow.
    pub fn shielded_rescan_plan(&self) -> ShieldedRescanPlan {
        let (plan, coins) = match &self.state {
            State::Ledger8(w) => (
                RescanPlanKind::Ledger8(w.shielded_rescan_plan()),
                w.rescan_coins(),
            ),
            State::Ledger9(w) => (
                RescanPlanKind::Ledger9(w.shielded_rescan_plan()),
                w.rescan_coins(),
            ),
        };
        ShieldedRescanPlan {
            seed: self.seed.clone(),
            coins,
            plan,
        }
    }

    /// Apply a completed shielded rescan to `self` and persist it when a
    /// storage directory is configured.
    ///
    /// The rebuilt shielded state and its cursor replace what the wallet
    /// holds, and registrations are carried over as [`Self::commit_resync`]
    /// carries them. Callers must keep resyncs from interleaving (see
    /// [`ShieldedRescanPlan::run`]). Everything else a resync writes (dust,
    /// unshielded, parameters, block context, pending reservations) is left
    /// alone here.
    pub fn commit_shielded_rescan(
        &mut self,
        commit: ShieldedRescanCommit,
    ) -> Result<(), WalletError> {
        match (&mut self.state, commit.0) {
            (State::Ledger8(w), RescanCommitKind::Ledger8(c)) => w.commit_shielded_rescan(c),
            (State::Ledger9(w), RescanCommitKind::Ledger9(c)) => w.commit_shielded_rescan(c),
            (State::Ledger8(_), RescanCommitKind::Ledger9(_)) => Err(WalletError::LedgerMismatch {
                expected: LedgerVersion::V8,
                found: LedgerVersion::V9,
            }),
            (State::Ledger9(_), RescanCommitKind::Ledger8(_)) => Err(WalletError::LedgerMismatch {
                expected: LedgerVersion::V9,
                found: LedgerVersion::V8,
            }),
        }
    }
}

/// Snapshot of everything a resync's replay phase consumes, taken from a
/// `&Wallet` by [`Wallet::resync_plan`].
///
/// Exists so callers that share a wallet across tasks (notably
/// `midnight_provider::MidnightProvider`, which reaches it through an
/// `Arc<dyn WalletFacade>`) can run the slow replay I/O **without holding any
/// wallet lock**: snapshot under a brief read lock, [`ResyncPlan::run`] the
/// replays lock-free, then apply the validated result under a brief write
/// lock via [`Wallet::commit_resync`]. Single-task callers can keep using
/// [`Wallet::resync`], which composes the same three steps.
///
/// The fields are clones of the wallet's cursors and replay state; taking a
/// plan does not mutate or lock anything beyond the `&self` borrow.
#[must_use = "run the plan with ResyncPlan::run, then apply it with Wallet::commit_resync"]
pub struct ResyncPlan(ResyncPlanKind);

enum ResyncPlanKind {
    Ledger8 {
        plan: ledger_8::ResyncPlan,
        identity: WalletIdentity,
    },
    Ledger9(ledger_9::ResyncPlan),
}

/// What a resync that crosses a hard fork needs beyond its plan, to build
/// the wallet again in the later generation.
struct WalletIdentity {
    seed: WalletSeed,
    network_id: String,
    storage_dir: Option<PathBuf>,
    chain_pin: Option<ChainPin>,
    last_block_height: i64,
}

impl ResyncPlan {
    /// Run the resync's replay phase: resume the three indexer subscriptions
    /// from the snapshotted cursors and fetch the latest block (chain time +
    /// ledger parameters), all without touching the wallet.
    ///
    /// Returns the validated [`ResyncCommit`] to apply with
    /// [`Wallet::commit_resync`]. On any replay or fetch error nothing was
    /// committed anywhere, so the wallet the plan was taken from is
    /// untouched.
    ///
    /// Callers that release the wallet lock between plan and commit must
    /// serialize resyncs themselves (two concurrent runs would replay from
    /// the same cursors and race their commits); the provider holds a
    /// dedicated resync mutex across plan → run → commit for this.
    ///
    /// When the chain moved to a later generation since the wallet's last
    /// sync, the replay crosses the fork. It applies the shielded events up
    /// to the fork and carries the shielded state across. Then it replays the
    /// Dust stream into an empty state of the later generation. It writes
    /// nothing, and the commit saves the crossed state.
    pub async fn run(self, indexer_url: &str) -> Result<ResyncCommit, WalletError> {
        match self.0 {
            ResyncPlanKind::Ledger9(plan) => Ok(ResyncCommit(ResyncCommitKind::Ledger9(
                plan.run(indexer_url).await?,
            ))),
            ResyncPlanKind::Ledger8 { plan, identity } => {
                let block = latest_block(indexer_url).await?;
                let block = if LedgerVersion::of_block(&block)? == LedgerVersion::V8 {
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
                cross(plan, identity, &block, indexer_url).await
            }
        }
    }
}

/// Replay a ledger 8 wallet's plan onto a chain that crossed to ledger 9,
/// and return the wallet again in ledger 9.
async fn cross(
    plan: ledger_8::ResyncPlan,
    identity: WalletIdentity,
    block: &midnight_indexer_client::Block,
    indexer_url: &str,
) -> Result<ResyncCommit, WalletError> {
    let ledger_8::ResyncPlan {
        unshielded_address,
        dust_wallet,
        dust_event_id,
        zswap_state,
        zswap_event_id,
        unshielded_utxos,
        last_tx_id,
        ..
    } = plan;
    let from = Resume {
        zswap: ZswapState::Ledger8(zswap_state),
        zswap_event_id,
        dust: DustState::Ledger8(dust_wallet),
        dust_event_id,
        unshielded_utxos,
        last_tx_id,
        last_block_height: identity.last_block_height,
    };
    let state = sync_on_chain(
        ChainSync {
            indexer_url,
            seed: &identity.seed,
            address: &unshielded_address,
            network_id: &identity.network_id,
            storage_dir: identity.storage_dir.as_deref(),
            save: false,
            chain_pin: identity.chain_pin,
        },
        block,
        Some(from),
        None,
    )
    .await?;
    match state {
        State::Ledger9(wallet) => Ok(ResyncCommit(ResyncCommitKind::Crossed(Box::new(wallet)))),
        State::Ledger8(_) => Err(WalletError::LedgerMismatch {
            expected: LedgerVersion::V9,
            found: LedgerVersion::V8,
        }),
    }
}

/// Validated results of a resync's replay tasks and latest-block fetch,
/// ready to be committed into a [`Wallet`] via [`Wallet::commit_resync`].
///
/// Produced only by [`ResyncPlan::run`]; the fields are private so a commit
/// can't be forged from un-validated data. Grouping the commit inputs also
/// makes the commit-and-persist sequence unit-testable without a live
/// indexer.
#[must_use = "apply with Wallet::commit_resync, or the completed replay is discarded"]
pub struct ResyncCommit(ResyncCommitKind);

enum ResyncCommitKind {
    Ledger8(ledger_8::ResyncCommit),
    Ledger9(ledger_9::ResyncCommit),
    /// A ledger 8 wallet's resync that crossed the fork to ledger 9: the
    /// wallet again, in ledger 9.
    Crossed(Box<ledger_9::Wallet>),
}

/// Snapshot of what a shielded rescan's replay consumes, taken from a
/// `&Wallet` by [`Wallet::shielded_rescan_plan`].
///
/// A rescan replays `zswapLedgerEvents` from its first event against a state
/// that starts empty and carries the wallet's registered coins (see
/// [`Wallet::watch_for_coin`]). Starting over is the point: a replay that
/// meets an output it cannot claim collapses that Merkle leaf, and a resync
/// resumes from the cursor rather than revisiting it, so a registration made
/// after the fact is only honoured by a replay that starts at zero.
///
/// The plan / run / commit split mirrors [`ResyncPlan`], and for the same
/// reason: a caller that shares the wallet across tasks snapshots under a
/// brief read lock, runs the replay lock-free, and applies the result under a
/// brief write lock. Single-task callers can use [`Wallet::rescan_shielded`],
/// which composes the three steps.
#[must_use = "run the plan with ShieldedRescanPlan::run, then apply it with Wallet::commit_shielded_rescan"]
pub struct ShieldedRescanPlan {
    seed: WalletSeed,
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
    /// [`Wallet::commit_shielded_rescan`]. On a replay error nothing was
    /// committed anywhere, so the wallet the plan was taken from is
    /// untouched.
    ///
    /// Callers that release the wallet lock between plan and commit must
    /// serialize this against resyncs (a resync committing its own cursor and
    /// state in the middle would overwrite the rebuilt state); the provider
    /// holds its resync mutex across plan → run → commit for this.
    ///
    /// A stream that starts in an earlier generation than the wallet's is
    /// replayed from that generation, and the state crosses each fork on the
    /// way.
    pub async fn run(self, indexer_url: &str) -> Result<ShieldedRescanCommit, WalletError> {
        let ShieldedRescanPlan { seed, coins, plan } = self;
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
                let start = ledger_8::ShieldedRescanPlan::new(&seed_8(&seed), &coins).initial_state;
                let sub_client = SubscriptionClient::new(indexer_url);
                let replayed = replay_zswap(
                    &sub_client,
                    &seed,
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
/// a [`Wallet`] via [`Wallet::commit_shielded_rescan`].
///
/// Produced only by [`ShieldedRescanPlan::run`]; the fields are private so a
/// commit can't be forged from un-replayed state.
#[must_use = "apply with Wallet::commit_shielded_rescan, or the completed replay is discarded"]
pub struct ShieldedRescanCommit(RescanCommitKind);

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
    seed: &WalletSeed,
    state: ZswapState,
    start_id: i64,
    resuming: bool,
    progress: Option<mpsc::Sender<SyncProgress>>,
) -> Result<ZswapReplayed, WalletError> {
    let (mut state, mut start_id) = (state, start_id);
    loop {
        let (replayed, later) = match state {
            ZswapState::Ledger8(zswap) => {
                let keys = ledger_8::state::zswap_keys(&seed_8(seed));
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
                let keys = ledger_9::state::zswap_keys(&seed_9(seed));
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
struct ChainSync<'a> {
    indexer_url: &'a str,
    seed: &'a WalletSeed,
    address: &'a str,
    network_id: &'a str,
    storage_dir: Option<&'a Path>,
    /// Whether the sync saves its state under `storage_dir` as it goes, or
    /// leaves that to a commit.
    save: bool,
    chain_pin: Option<ChainPin>,
}

/// One sync attempt: read the chain's generation, load the snapshot in its
/// own generation, and sync on the chain.
async fn sync_once(
    indexer_url: &str,
    seed: &WalletSeed,
    address: &str,
    network_id: &str,
    storage_dir: Option<&Path>,
    chain_pin: Option<ChainPin>,
    progress: Option<mpsc::Sender<SyncProgress>>,
) -> Result<State, WalletError> {
    let block = latest_block(indexer_url).await?;
    let chain = LedgerVersion::of_block(&block)?;
    let wallet_id = wallet_storage_id(address);

    info!("loading cached state from disk");
    let snapshot = match storage_dir {
        Some(dir) => load_snapshot(dir, network_id, &wallet_id, chain)?,
        None => None,
    };
    let (resume, chain_pin) = match snapshot {
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
            (Some(resume), chain_pin.or(stored_pin))
        }
        None => (None, chain_pin),
    };
    sync_on_chain(
        ChainSync {
            indexer_url,
            seed,
            address,
            network_id,
            storage_dir,
            save: true,
            chain_pin,
        },
        &block,
        resume,
        progress,
    )
    .await
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
) -> Result<State, WalletError> {
    let chain = LedgerVersion::of_block(block)?;
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
                ZswapState::Ledger8(ledger_8::state::empty_zswap_state(&seed_8(sync.seed))),
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
            sync.seed,
            zswap_start,
            start_zswap_id,
            resuming,
            progress.clone(),
        ),
        replay_unshielded_events(
            &sub_client,
            sync.address,
            utxos,
            start_tx_id,
            resuming,
            progress.clone(),
        ),
    );
    let zswap = zswap?;
    let (unshielded_utxos, last_tx_id, replay_block_height, spent_unshielded) = unshielded?;
    // The unshielded subscription only updates `last_block_height` when a
    // transaction touches our address. On a resume with no new unshielded
    // txs, replay returns 0, so we keep the persisted value as a floor.
    let last_block_height = replay_block_height.max(height);

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
                ledger_8::state::SyncIdentity {
                    indexer_url: sync.indexer_url,
                    seed: seed_8(sync.seed),
                    address: sync.address,
                    network_id: sync.network_id,
                    storage_dir: sync.storage_dir,
                    save: sync.save,
                    chain_pin: sync.chain_pin,
                },
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
            Ok(State::Ledger8(wallet))
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
                ledger_9::state::SyncIdentity {
                    indexer_url: sync.indexer_url,
                    seed: seed_9(sync.seed),
                    address: sync.address,
                    network_id: sync.network_id,
                    storage_dir: sync.storage_dir,
                    save: sync.save,
                    chain_pin: sync.chain_pin,
                },
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
            Ok(State::Ledger9(wallet))
        }
    }
}

#[cfg(test)]
mod tests {
    use midnight_helpers::{Tagged, ledger_8, ledger_9};
    use midnight_indexer_client::testutil::{accept_subscriber, bind, next_json, send_next};
    use serde_json::json;

    use super::*;

    fn seed() -> WalletSeed {
        WalletSeed::try_from_hex_str(&"22".repeat(32)).unwrap()
    }

    fn coin(nonce: u8) -> CoinInfo {
        CoinInfo {
            nonce: midnight_types::Nonce(midnight_types::HashOutput([nonce; 32])),
            type_: midnight_types::ShieldedTokenType(midnight_types::HashOutput([3; 32])),
            value: 42,
        }
    }

    /// The hex of `$ledger`'s event for an output of `$coin` to `seed()`'s
    /// wallet that carries no ciphertext, so only a registration claims it.
    macro_rules! output_event {
        ($ledger:ident, $coin:expr, $mt_index:expr) => {{
            use midnight_helpers::$ledger as l;
            use midnight_types::$ledger::convert::IntoLedger;
            let keys = crate::$ledger::state::zswap_keys(&IntoLedger::into_ledger(&seed()));
            let coin: l::CoinInfo = $coin.into_ledger();
            let event: l::Event<DefaultDB> = l::Event {
                source: l::mn_ledger::events::EventSource {
                    transaction_hash: l::TransactionHash(l::HashOutput([0; 32])),
                    logical_segment: 0,
                    physical_segment: 0,
                },
                content: l::mn_ledger::events::EventDetails::ZswapOutput {
                    commitment: coin.commitment(&l::Recipient::User(keys.coin_public_key())),
                    preimage_evidence: l::mn_ledger::events::ZswapPreimageEvidence::None,
                    contract: None,
                    mt_index: $mt_index,
                },
            };
            let mut raw = Vec::new();
            midnight_helpers::midnight_serialize::tagged_serialize(&event, &mut raw).unwrap();
            hex::encode(raw)
        }};
    }

    /// A chain that crossed the fork serves ledger 8 events, then ledger 9
    /// ones, in one stream. A replay that stopped at the boundary, or lost the
    /// state it built before it, would lose the coins of one half. The second
    /// coin is claimed by a registration made before the fork, which the
    /// crossing must carry.
    #[tokio::test]
    async fn a_zswap_replay_crosses_the_fork_with_the_coins_of_both_halves() {
        let (listener, url) = bind().await;
        let events = [
            (1, output_event!(ledger_8, coin(1), 0)),
            (2, output_event!(ledger_9, coin(2), 1)),
        ];
        let server = tokio::spawn(async move {
            // One subscription per generation the replay passes through.
            for _ in 0..2 {
                let (mut ws, sub) = accept_subscriber(&listener).await;
                let from = sub["payload"]["variables"]["id"].as_i64().unwrap();
                for (id, raw) in events.iter().filter(|(id, _)| *id >= from) {
                    let message = json!({"zswapLedgerEvents": {"id": id, "raw": raw, "maxId": 2}});
                    send_next(&mut ws, &sub, message).await;
                }
                while next_json(&mut ws).await.is_some() {}
            }
        });

        let registered =
            crate::ledger_8::state::ShieldedRescanPlan::new(&seed_8(&seed()), &[coin(1), coin(2)]);
        let replayed = replay_zswap(
            &SubscriptionClient::new(&url),
            &seed(),
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
                let message = json!({"dustLedgerEvents": {"id": id, "raw": raw, "maxId": 3}});
                send_next(&mut ws, &sub, message).await;
            }
            while next_json(&mut ws).await.is_some() {}
        });

        let wallet = ledger_9::DustWallet::default(seed(), Some(&ledger_9::INITIAL_PARAMETERS));
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
