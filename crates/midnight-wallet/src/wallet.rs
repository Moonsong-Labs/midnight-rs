//! A synced wallet, and the sync that builds it.

use std::path::{Path, PathBuf};

use midnight_indexer_client::SubscriptionClient;
use midnight_types::chain_pin::ChainPin;
use midnight_types::hd::Seed;
use midnight_types::{
    ChainParameters, CoinInfo, Network, ShieldedRecipient, SpendableShieldedCoin, SpentInputs,
    SyncCursors, TrackedUtxo, UnshieldedRecipient, WalletBalance, WalletError,
};
use tokio::sync::mpsc;
use tracing::info;

use crate::ledger_9::snapshot::LoadedState;
use crate::replay::{
    UnshieldedReplay, latest_block, progress_cancelled, replay_unshielded_events, require_ledger_9,
    send_progress,
};
use crate::storage::wallet_storage_id;
use crate::sync::SyncPlan;
use crate::{SyncProgress, ledger_9};

/// A Midnight wallet: identity (seed, addresses) and synced ledger state.
///
/// Maintains three streams of state from the indexer:
/// - `zswapLedgerEvents` → shielded coin tracking + Merkle tree
/// - `dustLedgerEvents` → dust/fee UTXO tracking
/// - `unshieldedTransactions` → unshielded UTXO balance
///
/// The wallet syncs with a chain that runs ledger 9. A sync from the first
/// event also reads the shielded events that the chain made before its hard
/// fork to ledger 9, so the wallet keeps the coins it received before the
/// fork.
pub struct Wallet {
    state: ledger_9::Wallet,
}

impl Wallet {
    pub(crate) fn state(&self) -> &ledger_9::Wallet {
        &self.state
    }

    pub(crate) fn state_mut(&mut self) -> &mut ledger_9::Wallet {
        &mut self.state
    }

    /// Default storage directory: `~/.midnight/wallets/`
    pub fn default_storage_dir() -> Option<PathBuf> {
        std::env::home_dir().map(|h| h.join(".midnight").join("wallets"))
    }

    /// The chain pin a snapshot on disk carries, or `None` when there is no
    /// snapshot or it predates the pin.
    ///
    /// Read this before a resume and judge it with
    /// [`crate::chain_pin::verify_pin`], against what the node reports for
    /// that height. A snapshot from a chain that no longer exists still
    /// resumes cleanly and reports the old balance, so nothing later in the
    /// sync will catch it.
    ///
    /// # Errors
    ///
    /// [`WalletError::Ledger8Snapshot`] when ledger 8 wrote the snapshot.
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

    /// The sync that [`crate::WalletBuilder::sync`] and
    /// [`crate::WalletBuilder::sync_with_progress`] run.
    ///
    /// Refuses a chain that does not run ledger 9, resumes from a snapshot
    /// under `storage_dir` when there is one, and replays the three event
    /// streams. Checkpoints Dust progress to disk periodically, so an
    /// interrupted sync resumes where it left off.
    pub(crate) async fn sync_inner(
        plan: SyncPlan,
        progress: Option<mpsc::Sender<SyncProgress>>,
    ) -> Result<Self, WalletError> {
        let sync = ChainSync {
            indexer_url: &plan.indexer_url,
            seed: &plan.seed,
            address: &plan.address,
            network: &plan.network,
            storage_dir: plan.storage_dir.as_deref(),
            chain_pin: plan.chain_pin,
        };
        let block = latest_block(sync.indexer_url).await?;
        require_ledger_9(&block)?;

        info!("loading cached state from disk");
        let snapshot = match sync.storage_dir {
            Some(dir) => ledger_9::snapshot::load(
                dir,
                sync.network.as_str(),
                &wallet_storage_id(sync.address),
            )?,
            None => None,
        };
        let sync = match &snapshot {
            Some(snapshot) => {
                info!(
                    zswap_event_id = snapshot.zswap_event_id,
                    dust_event_id = snapshot.dust_event_id,
                    "resuming from cached state"
                );
                let alive = send_progress(
                    &progress,
                    SyncProgress::Resuming {
                        zswap_event_id: snapshot.zswap_event_id,
                        dust_event_id: snapshot.dust_event_id,
                    },
                );
                if !alive {
                    return Err(progress_cancelled("resume"));
                }
                // Keep the snapshot's own pin when this sync has no fresher
                // one. A node that could not answer must not cost the wallet
                // the mark that lets the next resume check itself.
                let chain_pin = sync.chain_pin.or_else(|| snapshot.chain_pin.clone());
                ChainSync { chain_pin, ..sync }
            }
            None => sync,
        };
        let state = sync_on_chain(sync, &block, snapshot, progress).await?;
        Ok(Self { state })
    }

    /// Whether the dust state has been synced (required for transaction
    /// building).
    pub fn dust_synced(&self) -> bool {
        self.state.dust_synced()
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
        self.state.release(spent)
    }

    /// Keep the reservation of a proven build across a restart. See
    /// [`WalletFacade::persist_reservation`](midnight_wallet_facade::WalletFacade::persist_reservation).
    pub(crate) fn persist_reservation(&mut self, spent: &SpentInputs) {
        self.state.persist_reservation(spent)
    }

    /// Save the current wallet state to disk.
    ///
    /// Writes the confirmed-state files (`metadata.json`, `zswap-N.bin`,
    /// `dust_wallet-N.bin`) and the reservations of proven builds to a
    /// separate `pending.json`. Confirmed and pending live in distinct files
    /// so a failed save of one does not corrupt the other. Runs automatically at
    /// the end of initial sync and after every successful resync when a
    /// storage directory is configured; calling it manually is only needed
    /// for extra checkpoints.
    pub fn save(&self, base: &Path) -> Result<(), WalletError> {
        self.state.save(base)
    }

    /// Where this wallet's snapshot lives, when it persists one.
    ///
    /// An error that tells a reader to remove the snapshot has to name it, so
    /// this is the path that goes in the message.
    pub fn snapshot_dir(&self) -> Option<PathBuf> {
        self.state.snapshot_dir()
    }

    /// The finalized block this wallet is pinned to, held in memory.
    ///
    /// A resume checks the pin on disk; a wallet that stays attached has to
    /// check this one, because its cursors go just as stale when the chain is
    /// replaced underneath it.
    pub fn chain_pin(&self) -> Option<&ChainPin> {
        self.state.chain_pin()
    }

    /// Move the pin to the block a fresh check saw, so it stays inside an
    /// archive's retention window rather than ageing out of it.
    pub fn set_chain_pin(&mut self, pin: ChainPin) {
        self.state.set_chain_pin(pin)
    }

    /// How far the sync has reached, as one snapshot.
    ///
    /// The four cursors advance together during a sync, so reading them one
    /// at a time can report a mixture of two syncs.
    pub fn sync_cursors(&self) -> SyncCursors {
        self.state.sync_cursors()
    }

    /// The seed this wallet signs and derives with.
    pub fn seed(&self) -> &Seed {
        self.state.seed()
    }

    /// The wallet's shielded public keys: the coin public key an output
    /// commits to, and the encryption key its discovery ciphertext is sealed
    /// to.
    ///
    /// Public material, so anything that only needs to address a coin to this
    /// wallet can take these instead of the seed. A signer that never releases
    /// its seed can still supply them.
    pub fn shielded_recipient(&self) -> ShieldedRecipient {
        self.state.shielded_recipient()
    }

    /// The network this wallet derives addresses for. [`Network::as_str`]
    /// gives its literal name in the bech32 HRP.
    pub fn network(&self) -> &Network {
        self.state.network()
    }

    /// The wallet's unshielded receiving address.
    pub fn unshielded_address(&self) -> String {
        self.state.unshielded_address()
    }

    /// The wallet's user address, the recipient of an unshielded transfer to
    /// this wallet. See [`Seed::unshielded_recipient`].
    pub fn unshielded_recipient(&self) -> UnshieldedRecipient {
        self.seed().unshielded_recipient()
    }

    /// The wallet's shielded receiving address, e.g. `mn_shield-addr_undeployed1...`.
    pub fn shielded_address(&self) -> String {
        self.seed().shielded_address(self.network())
    }

    /// The wallet's Dust address, e.g. `mn_dust_undeployed1...`. Another
    /// wallet can register its tNIGHT to it, so that this wallet gets the
    /// Dust that tNIGHT generates. See [`Seed::dust_address`].
    pub fn dust_address(&self) -> String {
        self.seed().dust_address(self.network())
    }

    /// The unshielded UTXOs this wallet tracks.
    pub fn unshielded_utxos(&self) -> &[TrackedUtxo] {
        self.state.unshielded_utxos()
    }

    /// The chain's Dust and TTL parameters, as this wallet last synced them.
    pub fn parameters(&self) -> ChainParameters {
        self.state.chain_parameters()
    }

    pub fn balance(&self) -> WalletBalance {
        self.state.balance()
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
        self.state.spendable_shielded_coins()
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
    /// instead of revisiting it. Register through the handle instead:
    /// [`WalletFacade::watch_for_coins`](crate::WalletFacade::watch_for_coins)
    /// on a [`LocalWallet`](crate::LocalWallet) registers, then replays the
    /// stream from its first event. `MidnightProvider::watch_for_coin` calls
    /// it.
    ///
    /// A registration is part of the wallet state, so it is persisted with it
    /// when a storage directory is configured, and a coin registered before
    /// it lands on chain survives a restart. The in-memory registration
    /// stands even when that save fails.
    pub fn watch_for_coin(&mut self, coin: CoinInfo) -> Result<(), WalletError> {
        self.state.watch_for_coin(coin)
    }

    /// [`Self::watch_for_coin`] for several coins, persisting once.
    pub fn watch_for_coins(
        &mut self,
        coins: impl IntoIterator<Item = CoinInfo>,
    ) -> Result<(), WalletError> {
        self.state.watch_for_coins(coins)
    }

    /// Drop a registration [`Self::watch_for_coin`] made, for a coin that
    /// turned out to be wrong.
    ///
    /// A registration whose `CoinInfo` does not match the coin an on-chain
    /// output commits to never matches anything, so it would otherwise ride
    /// along on every later replay.
    ///
    /// This drops the registration only. A coin the wallet already claimed is
    /// untouched, and stays spendable: it is held now, not watched for.
    /// Forgetting a coin that was never registered changes no registration.
    /// When the last save failed, this call still saves, and can return
    /// [`WalletError::Storage`].
    pub fn forget_coin(&mut self, coin: CoinInfo) -> Result<(), WalletError> {
        self.state.forget_coin(coin)
    }

    /// [`Self::forget_coin`] for several coins, persisting once.
    pub fn forget_coins(
        &mut self,
        coins: impl IntoIterator<Item = CoinInfo>,
    ) -> Result<(), WalletError> {
        self.state.forget_coins(coins)
    }

    /// Snapshot the inputs of a resync's replay phase. See [`ResyncPlan`].
    pub(crate) fn resync_plan(&self) -> ResyncPlan {
        ResyncPlan {
            indexer_url: self.state.indexer_url().to_string(),
            plan: self.state.resync_plan(),
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
    /// - Coin registrations ([`Self::watch_for_coin`]) live in `zswap_state`.
    ///   The replay starts from the plan's registrations, so the commit keeps
    ///   each one that the replay did not consume. A replay consumes a
    ///   registration when it claims the coin. The overwrite drops a
    ///   registration made after the plan snapshot, and brings back one
    ///   forgotten after it. So callers must not change a registration
    ///   between plan and commit, as [`LocalWallet`](crate::LocalWallet)
    ///   makes sure with its sync lock.
    /// - The pending reservation set is **merged, not overwritten**:
    ///   `clear_confirmed` drops exactly the entries whose spends this
    ///   replay observed on-chain, evaluated against the pending set as it
    ///   is *now*. Reservations added after the plan snapshot survive (their
    ///   spends cannot have been observed by a replay that started earlier).
    /// - `parameters` and `block_context` are refreshed from the chain view
    ///   the replay fetched.
    pub(crate) fn commit_resync(
        &mut self,
        commit: ledger_9::ResyncCommit,
    ) -> Result<(), WalletError> {
        self.state.commit_resync(commit)
    }

    /// Snapshot the inputs of a shielded rescan's replay phase. See
    /// [`ShieldedRescanPlan`] for the intended plan → run → commit flow.
    pub(crate) fn shielded_rescan_plan(&self) -> ShieldedRescanPlan {
        ShieldedRescanPlan {
            indexer_url: self.state.indexer_url().to_string(),
            plan: self.state.shielded_rescan_plan(),
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
        commit: ledger_9::ShieldedRescanCommit,
    ) -> Result<(), WalletError> {
        self.state.commit_shielded_rescan(commit)
    }
}

/// Snapshot of everything a resync's replay phase consumes, taken from a
/// `&Wallet` by [`Wallet::resync_plan`], with the indexer the wallet synced
/// from.
///
/// Exists so callers that share a wallet across tasks (notably
/// [`LocalWallet`](crate::LocalWallet)) can run the slow replay I/O while
/// reads and builds still reach the wallet: snapshot under a brief read lock,
/// [`ResyncPlan::run`] the replays with no read or write lock held, then apply
/// the validated result under a brief write lock via
/// [`Wallet::commit_resync`].
///
/// The fields are clones of the wallet's cursors and replay state; taking a
/// plan does not mutate or lock anything beyond the `&self` borrow.
#[must_use = "run the plan with ResyncPlan::run, then apply it with Wallet::commit_resync"]
pub(crate) struct ResyncPlan {
    indexer_url: String,
    plan: ledger_9::ResyncPlan,
}

impl ResyncPlan {
    /// Run the resync's replay phase: resume the three indexer subscriptions
    /// from the snapshotted cursors and fetch the latest block (chain time +
    /// ledger parameters), all without touching the wallet.
    ///
    /// Returns the validated [`ledger_9::ResyncCommit`] to apply with
    /// [`Wallet::commit_resync`]. On any replay or fetch error nothing was
    /// committed anywhere, so the wallet the plan was taken from is
    /// untouched.
    ///
    /// Callers that release the wallet lock between plan and commit must
    /// serialize resyncs themselves (two concurrent runs would replay from
    /// the same cursors and race their commits).
    /// [`LocalWallet`](crate::LocalWallet) holds a mutex across plan → run →
    /// commit for this.
    pub(crate) async fn run(self) -> Result<ledger_9::ResyncCommit, WalletError> {
        self.plan.run(&self.indexer_url).await
    }
}

/// Snapshot of what a shielded rescan's replay consumes, taken from a
/// `&Wallet` by [`Wallet::shielded_rescan_plan`], with the indexer the wallet
/// synced from.
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
/// brief read lock, runs the replay with no read or write lock held, and
/// applies the result under a brief write lock.
#[must_use = "run the plan with ShieldedRescanPlan::run, then apply it with Wallet::commit_shielded_rescan"]
pub(crate) struct ShieldedRescanPlan {
    indexer_url: String,
    plan: ledger_9::ShieldedRescanPlan,
}

impl ShieldedRescanPlan {
    /// Run the rescan's replay phase: replay the whole shielded event stream
    /// from its first event, without touching the wallet.
    ///
    /// Returns the [`ledger_9::ShieldedRescanCommit`] to apply with
    /// [`Wallet::commit_shielded_rescan`]. On a replay error nothing was
    /// committed anywhere, so the wallet the plan was taken from is
    /// untouched.
    ///
    /// Callers that release the wallet lock between plan and commit must
    /// serialize this against resyncs (a resync committing its own cursor and
    /// state in the middle would overwrite the rebuilt state).
    /// [`LocalWallet`](crate::LocalWallet) holds a mutex across plan → run →
    /// commit for this.
    ///
    /// The replay reads the events before the chain's hard fork to ledger 9
    /// in ledger 8, and carries the state across the fork.
    pub(crate) async fn run(self) -> Result<ledger_9::ShieldedRescanCommit, WalletError> {
        self.plan.run(&self.indexer_url).await
    }
}

/// Who a sync is for, and where it keeps what it learns.
pub(crate) struct ChainSync<'a> {
    pub(crate) indexer_url: &'a str,
    pub(crate) seed: &'a Seed,
    pub(crate) address: &'a str,
    pub(crate) network: &'a Network,
    /// Where the sync writes its state as it goes: Dust checkpoints, the
    /// pending reservations it loads, and the synced state.
    pub(crate) storage_dir: Option<&'a Path>,
    /// The pin this sync persists, so a later resume can ask whether it is
    /// still on this chain.
    pub(crate) chain_pin: Option<ChainPin>,
}

/// Sync on a chain whose latest block is `block`, from `snapshot` or from
/// genesis. Replay the shielded and unshielded streams, then finish with the
/// Dust stream.
async fn sync_on_chain(
    sync: ChainSync<'_>,
    block: &midnight_indexer_client::Block,
    snapshot: Option<LoadedState>,
    progress: Option<mpsc::Sender<SyncProgress>>,
) -> Result<ledger_9::Wallet, WalletError> {
    // Where each stream starts. When resuming, start from the next event
    // after the last one applied; the subscription is inclusive, so the
    // last one itself would be delivered again.
    let (zswap_resume, dust, utxos, start_tx_id, height) = match snapshot {
        Some(s) => (
            Some((s.zswap_state, s.zswap_event_id + 1)),
            ledger_9::state::DustStart::Resume {
                wallet: Box::new(s.dust_wallet),
                next_id: s.dust_event_id + 1,
            },
            s.unshielded_utxos,
            s.last_tx_id.map(|id| id + 1).unwrap_or(0),
            s.last_block_height,
        ),
        None => (None, ledger_9::state::DustStart::Fresh, Vec::new(), 0, 0),
    };
    let resuming = zswap_resume.is_some();

    let sub_client = SubscriptionClient::new(sync.indexer_url);
    info!(
        start_zswap_id = zswap_resume.as_ref().map_or(0, |(_, id)| *id),
        start_tx_id, "starting subscriptions"
    );
    let zswap = async {
        match zswap_resume {
            Some((state, next_id)) => {
                ledger_9::state::replay_zswap_events(
                    &sub_client,
                    &ledger_9::state::zswap_keys(sync.seed),
                    state,
                    next_id,
                    true,
                    progress.clone(),
                )
                .await
            }
            None => {
                ledger_9::state::replay_zswap_from_genesis(
                    &sub_client,
                    sync.seed,
                    &[],
                    progress.clone(),
                )
                .await
            }
        }
    };
    let (zswap, unshielded) = tokio::join!(
        zswap,
        replay_unshielded_events(
            &sub_client,
            sync.address,
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

    ledger_9::Wallet::finish_sync(
        sync,
        block,
        ledger_9::state::SyncStart {
            zswap_state: zswap.state,
            zswap_event_id: zswap.last_id,
            unshielded_utxos,
            last_tx_id,
            last_block_height,
            spent_unshielded,
            dust,
        },
        progress,
    )
    .await
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
    use midnight_helpers::{DefaultDB, Tagged, ledger_8, ledger_9};
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
                crate::pre_fork::test_support::ciphertextless_output(
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
            // One subscription for the reader of the events before the fork,
            // and one for the ledger 9 replay after it.
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

        let replayed = crate::ledger_9::state::replay_zswap_from_genesis(
            &SubscriptionClient::new(&url),
            &test_seed(),
            &[unreachable_coin(1, 42), unreachable_coin(2, 42)],
            None,
        )
        .await
        .expect("the replay crosses the fork");
        server.await.unwrap();

        assert_eq!(replayed.last_id, 2);
        assert_eq!(
            replayed.state.coins.iter().count(),
            2,
            "a coin from each half"
        );
        assert_eq!(
            replayed.state.pending_outputs.iter().count(),
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
        assert!(replay.spend_nullifiers.is_empty());
    }

    /// The state files of a ledger 8 snapshot carry the same tags as those of
    /// ledger 9, so only its metadata tells them apart. The pin read and the
    /// load both refuse it, and the error names the directory to delete. The
    /// ledger 9 snapshots on disk hold `"V9"`, and they must keep loading.
    #[test]
    fn a_ledger_8_snapshot_is_refused_with_the_directory_to_delete() {
        let base = tempfile::TempDir::new().unwrap();
        let address = "mn_addr_undeployed1wallet";
        let dir = Wallet::snapshot_path(base.path(), Network::Undeployed, address);
        std::fs::create_dir_all(&dir).unwrap();
        let metadata = |ledger_version: &str| {
            format!(
                r#"{{
                    "generation": 1,
                    {ledger_version}
                    "zswap_event_id": 1,
                    "dust_event_id": 1,
                    "last_block_height": 0,
                    "last_tx_id": null,
                    "unshielded_utxos": []
                }}"#
            )
        };

        // A snapshot from before the SDK named the generation carries no
        // field, and one from after carries `"V8"`.
        for ledger_version in ["", r#""ledger_version": "V8","#] {
            std::fs::write(dir.join("metadata.json"), metadata(ledger_version)).unwrap();
            let pin_read = Wallet::stored_chain_pin(base.path(), Network::Undeployed, address)
                .expect_err("a ledger 8 snapshot must not give its pin");
            let load = crate::ledger_9::snapshot::load(
                base.path(),
                Network::Undeployed.as_str(),
                &wallet_storage_id(address),
            )
            .err()
            .expect("a ledger 8 snapshot must not load");
            for err in [pin_read, load] {
                assert!(
                    matches!(&err, WalletError::Ledger8Snapshot { path } if *path == dir.display().to_string()),
                    "{ledger_version:?}: {err}"
                );
            }
        }

        std::fs::write(
            dir.join("metadata.json"),
            metadata(r#""ledger_version": "V9","#),
        )
        .unwrap();
        Wallet::stored_chain_pin(base.path(), Network::Undeployed, address)
            .expect("a ledger 9 snapshot loads");
    }
}
