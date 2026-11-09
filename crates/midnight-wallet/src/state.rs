use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use midnight_helpers::midnight_serialize::tagged_deserialize;
use midnight_helpers::mn_ledger::events::EventDetails;
use midnight_helpers::mn_ledger::semantics::ZswapLocalStateExt;
use midnight_helpers::mn_ledger::structure::{Utxo as LedgerUtxo, UtxoMeta};
use midnight_helpers::{
    BlockContext, DefaultDB, DustNullifier, DustWallet, Event, LedgerContext, LedgerParameters,
    SecretKeys, ShieldedWallet, Sp, Timestamp, UnshieldedWallet, Wallet as ContextWallet,
    WalletState as ZswapLocalState,
};
use midnight_indexer_client::SubscriptionClient;
use midnight_types::build::{DustSpendBatch, SpentUtxoKey, TrackedUtxo, chain_time};
use midnight_types::convert::{IntoLedger, IntoSdk};
use tokio::sync::mpsc;
use tracing::{info, warn};

use super::pending::{PendingBook, PendingFile, PendingReservations, lock, within_ttl};
use crate::chain_pin::ChainPin;
use crate::generation::Generation;
use crate::replay::{
    LedgerStream, UnshieldedReplay, replay_ledger_events, replay_unshielded_events,
};
use crate::storage::wallet_storage_id;
use crate::wallet::ChainSync;
use crate::{Network, Seed, SyncProgress, WalletError};

/// A Midnight wallet: identity (seed, addresses) and synced ledger state.
///
/// Maintains three streams of state from the indexer:
/// - `zswapLedgerEvents` → shielded coin tracking + Merkle tree
/// - `dustLedgerEvents` → dust/fee UTXO tracking
/// - `unshieldedTransactions` → unshielded UTXO balance
///
/// The state of the SDK's `Wallet`. Transaction building uses it directly (no
/// full-chain replay).
pub struct Wallet {
    seed: Seed,
    secret_keys: SecretKeys,
    network: Network,
    unshielded_address: String,

    // Shielded state (from zswapLedgerEvents)
    zswap_state: ZswapLocalState<DefaultDB>,
    zswap_event_id: i64,

    // Dust state (from dustLedgerEvents)
    dust_wallet: DustWallet<DefaultDB>,
    dust_event_id: i64,

    // Unshielded UTXOs (from unshieldedTransactions)
    unshielded_utxos: Vec<TrackedUtxo>,
    last_block_height: i64,
    last_tx_id: Option<i64>,

    /// The finalized block this wallet's snapshot pins, so a later resume can
    /// tell whether it is still on the same chain. The node supplies it; see
    /// [`crate::chain_pin`].
    chain_pin: Option<ChainPin>,

    // Chain parameters (from latest block via indexer HTTP)
    parameters: LedgerParameters,
    /// The context a build runs at: `tblock` is the Dust anchor
    /// ([`dust_anchor`]), and `last_block_time` the time of the chain's latest
    /// block, which a build counts its TTL from. Both differ from the ledger's
    /// meaning of the fields; see [`chain_time`].
    block_context: Option<BlockContext>,

    /// In-flight reservations: spends built locally but not yet observed
    /// as confirmed on-chain. Applied at [`Wallet::add_funding`] time to
    /// prevent local double-builds, cleared when corresponding events arrive
    /// or when the TTL window elapses. Never written to the confirmed-state
    /// files; persisted separately via `pending.json`.
    pending: Arc<Mutex<PendingBook>>,

    /// Where this wallet persists its state, when [`Wallet::sync_inner`] was
    /// given a storage directory. Retained so [`Wallet::commit_resync`] can
    /// re-save the moved cursors without the caller re-supplying the path.
    storage_dir: Option<PathBuf>,

    /// The indexer this wallet synced against.
    ///
    /// The cursors are event counts, so they mean nothing against a different
    /// server: a replay resumed elsewhere would apply another indexer's events
    /// to this state. Holding the URL here keeps every later replay on the
    /// wallet's own indexer rather than one a consumer supplies.
    ///
    /// It binds nothing across a restart. The snapshot records no indexer
    /// identity, so a sync from a source on another indexer, with
    /// `.with_storage(dir)`, resumes these cursors against that indexer and
    /// nothing notices.
    indexer_url: String,
}

// ---------------------------------------------------------------------------
// Wallet implementation
// ---------------------------------------------------------------------------

/// Number of dust events between checkpoint saves during initial sync.
const DUST_CHECKPOINT_INTERVAL: u64 = 50_000;

type DustCheckpointFn = dyn Fn(&DustWallet<DefaultDB>, i64) + Send;

/// What a mid-sync checkpoint writes beside the dust wallet it is handed.
///
/// Owned, because the checkpoint closure outlives the frame that builds it,
/// so this cannot borrow the way [`super::snapshot::Snapshot`] does.
struct CheckpointState {
    wallet_id: String,
    zswap_state: ZswapLocalState<DefaultDB>,
    zswap_event_id: i64,
    last_block_height: i64,
    last_tx_id: Option<i64>,
    chain_pin: Option<ChainPin>,
    unshielded_utxos: Vec<TrackedUtxo>,
}

fn make_dust_checkpoint(
    storage_dir: Option<&Path>,
    network_id: &str,
    state: CheckpointState,
) -> Option<Box<DustCheckpointFn>> {
    let CheckpointState {
        wallet_id,
        zswap_state,
        zswap_event_id,
        last_block_height,
        last_tx_id,
        chain_pin,
        unshielded_utxos,
    } = state;
    storage_dir.map(|dir| {
        let dir = dir.to_path_buf();
        let net = network_id.to_string();
        Box::new(move |dw: &DustWallet<DefaultDB>, dust_eid: i64| {
            if let Err(err) = super::snapshot::save(
                &dir,
                &net,
                &wallet_id,
                super::snapshot::Snapshot {
                    zswap_state: &zswap_state,
                    dust_wallet: dw,
                    zswap_event_id,
                    dust_event_id: dust_eid,
                    last_block_height,
                    last_tx_id,
                    chain_pin: chain_pin.as_ref(),
                    unshielded_utxos: &unshielded_utxos,
                },
            ) {
                warn!(error = %err, "failed to checkpoint dust state");
            }
        }) as Box<DustCheckpointFn>
    })
}

/// The `tblock` that a wallet anchors its block context at, after a replay
/// whose last Dust event carried `last_dust_block_time`.
///
/// `tblock` sets the proof's `DustActions.ctime`. A build counts the TTL of
/// its intents from `chain_now`, which the block context keeps beside the
/// anchor as its `last_block_time`. The chain checks two things of `ctime`:
///
/// 1. `root_history.get(ctime)` matches the wallet's Dust root. The chain
///    keeps `root_history` only for `global_ttl`: each block drops the
///    entries before `tblock - global_ttl`, so an older `ctime` resolves to no
///    root.
/// 2. `ctime + dust_grace_period >= chain.current_tblock` when it applies the
///    transaction.
///
/// Check 1 wants the most recent time that the chain resolves to the wallet's
/// root: `last_dust_block_time + 1s`. The root history changes only on a Dust
/// event, so every time in the gap after the last one resolves to it. Check 2
/// wants a `tblock` near the chain's current time `chain_now`. On devnet the
/// genesis is hardcoded months before wall clock while the chain runs in real
/// time, so a Dust time from a genesis event is too old for it.
///
/// So the anchor is the newest race-safe candidate while the smaller of
/// `global_ttl` and `dust_grace_period` still covers it:
/// `last_dust_block_time + 1s`, or `previous` when the replay saw no Dust
/// event. Otherwise it is `chain_now`. That fallback accepts a small race (a
/// Dust event indexed between the replay's tip and the latest block query),
/// but a chain whose time moved past the candidate's window needs it.
///
/// `previous` is race-safe only when no Dust event changed the wallet's state
/// after it was taken.
fn dust_anchor(
    last_dust_block_time: Option<Timestamp>,
    previous: Option<Timestamp>,
    chain_now: Timestamp,
    parameters: &LedgerParameters,
) -> Timestamp {
    // The chain enforces both bounds. The Dust grace period is often much
    // shorter, and it rejects a stale `ctime` with `OutOfDustValidityWindow`.
    let window = parameters.global_ttl.min(parameters.dust.dust_grace_period);
    match last_dust_block_time
        .map(|t| t + midnight_helpers::Duration::from_secs(1))
        .or(previous)
    {
        Some(t) if t + window >= chain_now => t,
        _ => chain_now,
    }
}

/// Tagged-deserialize a ledger 9 event.
fn decode_event(raw: &[u8], kind: &str) -> Result<Event<DefaultDB>, WalletError> {
    tagged_deserialize(raw).map_err(|e| WalletError::Sync(format!("deserialize {kind} event: {e}")))
}

/// Hex-decode and tagged-deserialize the `ledger_parameters` carried on an
/// indexer block. Both initial sync and resync read parameters from the
/// latest block so governance changes (fees, TTL, dust rates) take effect.
fn decode_ledger_parameters(
    block: &midnight_indexer_client::Block,
) -> Result<LedgerParameters, WalletError> {
    let params_bytes = crate::replay::encoded_ledger_parameters(block)?;
    let parameters: LedgerParameters = tagged_deserialize(&params_bytes[..])
        .map_err(|e| WalletError::Sync(format!("deserialize ledger params: {e}")))?;
    validate_ledger_parameters(&parameters)?;
    Ok(parameters)
}

/// The time of an indexer block, which the indexer gives in milliseconds.
fn block_time(block: &midnight_indexer_client::Block) -> Result<Timestamp, WalletError> {
    let ms = block
        .timestamp
        .ok_or_else(|| WalletError::Sync("latest block has no timestamp".into()))?;
    Ok(Timestamp::from_secs((ms / 1000) as u64))
}

/// Reject decoded ledger parameters the wallet's own math cannot sensibly
/// consume. Deserialization is purely structural, so a corrupt or hostile
/// indexer can deliver a well-formed blob full of zeros; without these
/// checks the wallet would compute nonsense fees and TTLs from it. The
/// checks are deliberately minimal: only fields the wallet actually
/// reads, with values no live chain can have.
///
/// - `global_ttl` anchors every transaction's validity window and pending
///   reservation eviction; a non-positive TTL makes every transaction
///   instantly expired.
/// - `dust.night_dust_ratio` scales NIGHT to dust capacity in the fee
///   availability math; zero means no fee could ever be paid.
/// - `dust.generation_decay_rate` is a divisor in the ledger's dust cap
///   math (`DustParameters::time_to_cap`); zero divides by zero.
/// - `fee_prices.overall_price` is the base price every fee dimension
///   scales from (`fees_with_margin`); non-positive prices every
///   transaction at zero dust.
fn validate_ledger_parameters(p: &LedgerParameters) -> Result<(), WalletError> {
    use midnight_helpers::base_crypto::cost_model::FixedPoint;

    let corrupt =
        |field: &'static str, value: String| WalletError::CorruptParameters { field, value };
    if p.global_ttl.as_seconds() <= 0 {
        return Err(corrupt("global_ttl", p.global_ttl.as_seconds().to_string()));
    }
    if p.dust.night_dust_ratio == 0 {
        return Err(corrupt(
            "dust.night_dust_ratio",
            p.dust.night_dust_ratio.to_string(),
        ));
    }
    if p.dust.generation_decay_rate == 0 {
        return Err(corrupt(
            "dust.generation_decay_rate",
            p.dust.generation_decay_rate.to_string(),
        ));
    }
    if p.fee_prices.overall_price <= FixedPoint::ZERO {
        return Err(corrupt(
            "fee_prices.overall_price",
            f64::from(p.fee_prices.overall_price).to_string(),
        ));
    }
    Ok(())
}

/// The wallet's half of [`crate::wallet::ResyncPlan`].
pub struct ResyncPlan {
    secret_keys: SecretKeys,
    unshielded_address: String,
    dust_wallet: DustWallet<DefaultDB>,
    dust_event_id: i64,
    zswap_state: ZswapLocalState<DefaultDB>,
    zswap_event_id: i64,
    unshielded_utxos: Vec<TrackedUtxo>,
    last_tx_id: Option<i64>,
    last_block_height: i64,
}

impl ResyncPlan {
    /// The replay phase of [`crate::wallet::ResyncPlan::run`].
    pub async fn run(self, indexer_url: &str) -> Result<ResyncCommit, WalletError> {
        let ResyncPlan {
            secret_keys,
            unshielded_address,
            dust_wallet,
            dust_event_id,
            zswap_state,
            zswap_event_id,
            unshielded_utxos,
            last_tx_id,
            last_block_height,
        } = self;

        let sub_client = SubscriptionClient::new(indexer_url);

        let start_tx_id = last_tx_id.map(|id| id + 1).unwrap_or(0);

        let (dust_res, zswap_res, unshielded_res, block_res) = tokio::join!(
            replay_dust_events(
                &sub_client,
                dust_wallet,
                dust_event_id + 1,
                true,
                None::<fn(&DustWallet<DefaultDB>, i64)>,
                None,
            ),
            replay_zswap_events(
                &sub_client,
                &secret_keys,
                zswap_state,
                zswap_event_id + 1,
                true,
                None,
            ),
            replay_unshielded_events(
                &sub_client,
                &unshielded_address,
                unshielded_utxos,
                start_tx_id,
                last_block_height,
                // A resume, like the two replays above: the wallet already
                // holds a cursor, so a stream with nothing to deliver means
                // this address is at the tip. An address with no new
                // transactions is the ordinary case, and reading its silence
                // as a failure ends the resync.
                true,
                None,
            ),
            crate::replay::latest_block(indexer_url),
        );

        // Await every result before returning. If any task failed, no commit
        // is produced and the source wallet stays as it was.
        let dust = dust_res?;
        let zswap = zswap_res?;
        let UnshieldedReplay {
            utxos: unshielded_utxos,
            last_tx_id,
            last_block_height,
            spent: spent_unshielded,
        } = unshielded_res?;
        let block = block_res?;
        crate::replay::require_ledger_9(&block)?;
        let DustReplay {
            wallet: dust_wallet,
            last_id: dust_event_id,
            last_block_time: last_dust_block_time,
            spend_nullifiers: dust_nullifiers,
        } = dust;
        let ZswapReplay {
            state: zswap_state,
            last_id: zswap_event_id,
        } = zswap;
        let chain_tblock = block_time(&block)?;
        let parameters = decode_ledger_parameters(&block)?;

        Ok(ResyncCommit {
            dust_wallet,
            dust_event_id,
            last_dust_block_time,
            dust_nullifiers,
            zswap_state,
            zswap_event_id,
            unshielded_utxos,
            last_tx_id,
            last_block_height,
            spent_unshielded,
            chain_tblock,
            parameters,
        })
    }
}

/// Validated results of a resync's replay tasks and latest-block fetch,
/// ready to be committed into a [`crate::wallet::WalletState`] via
/// [`crate::wallet::WalletState::commit_resync`].
///
/// Produced only by [`ResyncPlan::run`]; the fields are private so a commit
/// can't be forged from un-validated data. Grouping the commit inputs also
/// makes the commit-and-persist sequence unit-testable without a live
/// indexer.
#[must_use = "apply with Wallet::commit_resync, or the completed replay is discarded"]
pub struct ResyncCommit {
    dust_wallet: DustWallet<DefaultDB>,
    dust_event_id: i64,
    last_dust_block_time: Option<Timestamp>,
    dust_nullifiers: Vec<DustNullifier>,
    zswap_state: ZswapLocalState<DefaultDB>,
    zswap_event_id: i64,
    unshielded_utxos: Vec<TrackedUtxo>,
    last_tx_id: i64,
    last_block_height: i64,
    spent_unshielded: Vec<SpentUtxoKey>,
    chain_tblock: Timestamp,
    parameters: LedgerParameters,
}

/// The wallet's half of [`crate::wallet::ShieldedRescanPlan`]: who the
/// rescan is for, and the coins it claims on the way.
pub struct ShieldedRescanPlan {
    seed: Seed,
    coins: Vec<crate::CoinInfo>,
}

impl ShieldedRescanPlan {
    /// The replay of [`crate::wallet::ShieldedRescanPlan::run`], from the
    /// first event of the stream.
    pub async fn run(self, indexer_url: &str) -> Result<ShieldedRescanCommit, WalletError> {
        let sub_client = SubscriptionClient::new(indexer_url);
        let replay = replay_zswap_from_genesis(&sub_client, &self.seed, &self.coins, None).await?;
        Ok(ShieldedRescanCommit {
            zswap_state: replay.state,
            zswap_event_id: replay.last_id,
        })
    }
}

/// Validated result of a shielded rescan's replay, ready to be committed into
/// a [`crate::wallet::WalletState`] via
/// [`crate::wallet::WalletState::commit_shielded_rescan`].
///
/// Produced only by [`ShieldedRescanPlan::run`]; the fields are private so a
/// commit can't be forged from un-replayed state.
#[must_use = "apply with Wallet::commit_shielded_rescan, or the completed replay is discarded"]
pub struct ShieldedRescanCommit {
    zswap_state: ZswapLocalState<DefaultDB>,
    zswap_event_id: i64,
}

/// What a sync replayed before its Dust replay.
pub(crate) struct SyncStart {
    pub zswap_state: ZswapLocalState<DefaultDB>,
    pub zswap_event_id: i64,
    pub unshielded_utxos: Vec<TrackedUtxo>,
    pub last_tx_id: i64,
    pub last_block_height: i64,
    pub spent_unshielded: Vec<SpentUtxoKey>,
    pub dust: DustStart,
}

/// Where a sync's Dust replay starts.
pub(crate) enum DustStart {
    /// From a snapshot's Dust state, at the event after the last it applied.
    Resume {
        wallet: Box<DustWallet<DefaultDB>>,
        next_id: i64,
    },
    /// From an empty Dust state, at the first event: a sync with no snapshot.
    Fresh,
}

/// The shielded keys `seed` derives.
pub(crate) fn zswap_keys(seed: &Seed) -> SecretKeys {
    ShieldedWallet::<DefaultDB>::default(seed.into_ledger())
        .secret_keys()
        .clone()
}

impl Wallet {
    /// See [`crate::wallet::WalletState::snapshot_dir`].
    pub fn snapshot_dir(&self) -> Option<std::path::PathBuf> {
        let dir = self.storage_dir.as_deref()?;
        Some(crate::storage::snapshot_path(
            dir,
            self.network.as_str(),
            &wallet_storage_id(&self.unshielded_address),
        ))
    }

    /// See [`crate::wallet::WalletState::chain_pin`].
    pub fn chain_pin(&self) -> Option<&ChainPin> {
        self.chain_pin.as_ref()
    }

    /// The indexer this wallet synced from, and the only one its cursors
    /// mean anything against.
    pub fn indexer_url(&self) -> &str {
        &self.indexer_url
    }

    /// See [`crate::wallet::WalletState::set_chain_pin`].
    pub fn set_chain_pin(&mut self, pin: ChainPin) {
        self.chain_pin = Some(pin);
    }

    /// Finish a sync: replay the Dust stream from the cursor `start` carries,
    /// and assemble the wallet.
    ///
    /// The sync has replayed the shielded and unshielded streams already.
    /// `block` is the chain's latest block.
    pub(crate) async fn finish_sync(
        sync: ChainSync<'_>,
        block: &midnight_indexer_client::Block,
        start: SyncStart,
        progress: Option<mpsc::Sender<SyncProgress>>,
    ) -> Result<Self, WalletError> {
        let ChainSync {
            indexer_url,
            seed,
            address,
            network,
            storage_dir,
            chain_pin,
        } = sync;
        let SyncStart {
            zswap_state,
            zswap_event_id,
            unshielded_utxos,
            last_tx_id,
            last_block_height,
            spent_unshielded,
            dust,
        } = start;
        let wallet_id = wallet_storage_id(address);
        let secret_keys = zswap_keys(seed);

        let parameters = decode_ledger_parameters(block)?;
        let block_timestamp = block_time(block)?;

        let (dust_wallet, start_dust_id) = match dust {
            DustStart::Resume { wallet, next_id } => (*wallet, next_id),
            DustStart::Fresh => (
                DustWallet::default(seed.into_ledger(), Some(&parameters)),
                0,
            ),
        };

        let sub_client = SubscriptionClient::new(indexer_url);
        let dust_checkpoint = make_dust_checkpoint(
            storage_dir,
            network.as_str(),
            CheckpointState {
                wallet_id: wallet_id.clone(),
                zswap_state: zswap_state.clone(),
                zswap_event_id,
                last_block_height,
                last_tx_id: Some(last_tx_id),
                chain_pin: chain_pin.clone(),
                unshielded_utxos: unshielded_utxos.clone(),
            },
        );
        let dust_resuming = start_dust_id > 0;
        let dust = replay_dust_events(
            &sub_client,
            dust_wallet,
            start_dust_id,
            dust_resuming,
            dust_checkpoint,
            progress.clone(),
        )
        .await?;
        let DustReplay {
            wallet: dust_wallet,
            last_id: dust_event_id,
            last_block_time: last_dust_block_time,
            spend_nullifiers: dust_nullifiers,
        } = dust;

        let tblock = dust_anchor(last_dust_block_time, None, block_timestamp, &parameters);
        let block_context = Some(midnight_helpers::make_block_context(
            tblock,
            Default::default(),
            block_timestamp,
        ));

        info!(
            zswap_event_id,
            dust_event_id,
            unshielded_utxos = unshielded_utxos.len(),
            height = last_block_height,
            "wallet synced"
        );

        // Load any pre-existing pending reservations from disk so they
        // survive process restarts. Confirmed-state files never carry
        // pending entries; this is a separate file.
        let reservations = match storage_dir {
            Some(dir) => {
                PendingReservations::load(dir, network.as_str(), &wallet_id)?.unwrap_or_default()
            }
            None => PendingReservations::default(),
        };
        let file = storage_dir.map(|dir| PendingFile {
            dir: dir.to_path_buf(),
            network: network.as_str().to_owned(),
            wallet_id: wallet_id.clone(),
        });

        let state = Self {
            seed: seed.clone(),
            secret_keys,
            network: network.clone(),
            unshielded_address: address.to_string(),
            zswap_state,
            zswap_event_id,
            dust_wallet,
            dust_event_id,
            unshielded_utxos,
            last_block_height,
            last_tx_id: Some(last_tx_id),
            chain_pin,
            parameters,
            block_context,
            pending: Arc::new(Mutex::new(PendingBook::new(reservations, file))),
            storage_dir: storage_dir.map(Path::to_path_buf),
            indexer_url: indexer_url.to_string(),
        };

        {
            let mut book = lock(&state.pending);
            // Reservations made before a restart whose spends this replay just
            // observed confirmed are no longer in flight; drop them so the
            // underlying UTXOs become spendable again immediately.
            book.reservations_mut()
                .clear_confirmed(&spent_unshielded, &dust_nullifiers);

            // Any pending entry whose TTL has passed against the chain's
            // current view can no longer produce a valid transaction; drop
            // them so they don't pollute subsequent build contexts.
            if let Some(ref bc) = state.block_context {
                book.reservations_mut().evict_expired(chain_time(bc));
            }
        }

        if let Some(dir) = storage_dir {
            state.save(dir)?;
        }

        Ok(state)
    }

    /// Record the dust + unshielded spends of a freshly-built (and typically
    /// about-to-be-submitted) transaction so subsequent in-process builds
    /// don't re-select the same inputs.
    ///
    /// Dust and unshielded reservations live in the pending book until either:
    /// - event replay (a sync or a resync) observes the corresponding
    ///   confirmed spends and clears them,
    /// - or the chain time passes `expires_at` (evicted at
    ///   [`Wallet::add_funding`] time).
    ///
    /// `shielded_spends` (Zswap coin nullifiers pinned by a contract call) are
    /// cleared by TTL only: once the spend confirms, resync drops the coin from
    /// `zswap_state`, so filtering it out becomes a no-op regardless.
    ///
    /// `reserved_at` should be the [`chain_time`] of the build context
    /// (typically [`PreparedTransfer::reserved_at`](midnight_types::build::PreparedTransfer::reserved_at)),
    /// which a release names. `expires_at` should be the TTL of the
    /// transaction ([`PreparedTransfer::expires_at`](midnight_types::build::PreparedTransfer::expires_at)):
    /// TTL eviction compares it against the chain time of `block_context`.
    /// Confirmed-state files never persist these reservations.
    ///
    /// The reservation stays in memory until [`PendingBook::mark_proven`]
    /// names its build as proven, and only then reaches `pending.json`. A
    /// process that stops during the proof leaves no reservation on disk, so
    /// the next process can select these inputs again.
    pub fn reserve_pending(
        &mut self,
        dust_batches: Vec<DustSpendBatch>,
        unshielded_spends: Vec<SpentUtxoKey>,
        shielded_spends: Vec<midnight_helpers::Nullifier>,
        reserved_at: Timestamp,
        expires_at: Timestamp,
    ) {
        lock(&self.pending).reservations_mut().reserve(
            dust_batches,
            unshielded_spends,
            shielded_spends,
            reserved_at,
            expires_at,
        );
    }

    /// The book that holds this wallet's reservations.
    pub(crate) fn pending_book(&self) -> &Arc<Mutex<PendingBook>> {
        &self.pending
    }

    /// This wallet's on-disk identity; see [`wallet_storage_id`].
    fn storage_id(&self) -> String {
        wallet_storage_id(&self.unshielded_address)
    }

    /// Whether a reservation that expires at `expires_at` still holds its
    /// inputs.
    ///
    /// A reservation outside [`within_ttl`] at the chain time of
    /// `block_context` does not, because [`Self::add_funding`] evicts it before
    /// a build selects. With no block context, every reservation holds, as no
    /// eviction runs.
    fn reservation_holds(&self, expires_at: Timestamp) -> bool {
        self.block_context
            .as_ref()
            .is_none_or(|bc| within_ttl(expires_at, chain_time(bc)))
    }

    /// Nullifiers of the shielded coins that a reservation holds; see
    /// [`Self::reservation_holds`].
    pub(crate) fn reserved_shielded_nullifiers(&self) -> Vec<midnight_helpers::Nullifier> {
        lock(&self.pending)
            .reservations()
            .shielded_spends()
            .filter(|spend| self.reservation_holds(spend.expires_at))
            .map(|spend| spend.nullifier)
            .collect()
    }

    /// Keys of the unshielded UTXOs that a reservation holds; see
    /// [`Self::reservation_holds`].
    pub(crate) fn reserved_unshielded_keys(&self) -> Vec<SpentUtxoKey> {
        lock(&self.pending)
            .reservations()
            .unshielded_spends()
            .filter(|spend| self.reservation_holds(spend.expires_at))
            .map(|spend| spend.key.clone())
            .collect()
    }

    /// Nullifiers of the Dust UTXOs that a reservation holds; see
    /// [`Self::reservation_holds`].
    fn reserved_dust_nullifiers(&self) -> Vec<DustNullifier> {
        lock(&self.pending)
            .reservations()
            .dust_batches()
            .filter(|batch| self.reservation_holds(batch.expires_at))
            .flat_map(|batch| batch.nullifiers.iter().copied())
            .collect()
    }

    /// The confirmed Dust state less the UTXOs that a reservation holds; see
    /// [`Self::reservation_holds`]. `None` when the wallet has no Dust state.
    ///
    /// The spendable Dust of a balance and the Dust that a build selects from
    /// both come from this state, so the two agree. It keeps the confirmed
    /// commitment tree, because a Dust spend proves against a root that the
    /// chain holds in its root history.
    pub(super) fn unreserved_dust(&self) -> Option<midnight_helpers::DustLocalState<DefaultDB>> {
        let confirmed = self.dust_wallet.dust_local_state.as_deref()?;
        Some(
            self.reserved_dust_nullifiers()
                .iter()
                .fold(confirmed.clone(), |state, nullifier| {
                    state
                        .remove_utxo(nullifier)
                        .expect("the ledger's remove_utxo always returns Ok")
                }),
        )
    }

    /// Save the current wallet state to disk.
    ///
    /// Writes the confirmed-state files (`metadata.json`, `zswap-N.bin`,
    /// `dust_wallet-N.bin`) and the reservations of proven builds to a
    /// separate `pending.json`. Confirmed and pending live in distinct files
    /// so a failed save of one does not corrupt the other. Runs automatically
    /// at the end of initial sync and after every successful resync when a
    /// storage directory is configured.
    pub fn save(&self, base: &Path) -> Result<(), WalletError> {
        let wallet_id = self.storage_id();
        super::snapshot::save(
            base,
            self.network.as_str(),
            &wallet_id,
            super::snapshot::Snapshot {
                zswap_state: &self.zswap_state,
                dust_wallet: &self.dust_wallet,
                zswap_event_id: self.zswap_event_id,
                dust_event_id: self.dust_event_id,
                last_block_height: self.last_block_height,
                last_tx_id: self.last_tx_id,
                chain_pin: self.chain_pin.as_ref(),
                unshielded_utxos: &self.unshielded_utxos,
            },
        )?;
        lock(&self.pending)
            .reservations()
            .save(base, self.network.as_str(), &wallet_id)
    }

    /// Save to the storage directory when `changed` is true or the last save
    /// failed. A wallet with no storage directory saves nothing.
    ///
    /// A failed save leaves the memory ahead of the disk, so the next call
    /// retries it even when nothing changed.
    fn persist(&mut self, changed: bool) -> Result<(), WalletError> {
        if !(changed || lock(&self.pending).save_failed()) {
            return Ok(());
        }
        let Some(dir) = self.storage_dir() else {
            return Ok(());
        };
        let saved = self.save(dir);
        lock(&self.pending).set_save_failed(saved.is_err());
        saved
    }

    /// Put this wallet's spendable view into `ctx`: its unshielded UTXOs, its
    /// shielded coins, and its dust, each less what a still-pending build
    /// reserved.
    ///
    /// A build draws on this only for the seeds named in
    /// `StandardTransactionInfo::set_funding_seeds`, so a context that never
    /// gets this call funds nothing.
    ///
    /// Call this once per context, and errors on a second call for the same
    /// wallet. It also moves the block time of `ctx`'s block context to this
    /// wallet's Dust anchor, which a build reads as the Dust `ctime`, so the
    /// Dust spends witness a root that the chain holds at that time. The time
    /// of the last block stays as `ctx` has it, so a build still counts its
    /// TTL from the chain time of `ctx`. A `ctx` with no block context takes
    /// this wallet's whole.
    ///
    /// The only mutation is TTL eviction of expired `pending` entries against
    /// the chain time of `block_context`: entries whose `expires_at` has
    /// passed cannot produce a valid transaction, and would block the
    /// underlying UTXOs forever otherwise.
    pub fn add_funding(&mut self, ctx: &LedgerContext<DefaultDB>) -> Result<(), WalletError> {
        // One funding view per context. Funding twice would merge a fresh UTXO
        // set into the one already there: the second pass skips what a pending
        // build reserved, but cannot withdraw what the first pass inserted, so
        // a spent input stays selectable.
        let seed = (&self.seed).into_ledger();
        if ctx
            .wallets
            .lock()
            .map_err(|_| WalletError::Sync("wallets lock poisoned".into()))?
            .contains_key(&seed)
        {
            return Err(WalletError::Transfer(
                "this wallet already funds the context; build a fresh one".into(),
            ));
        }

        // Evict any expired pending reservations against the latest known
        // chain time. Cheap (Vec::retain on a typically tiny list) and the
        // only place that doesn't require the caller to restart the process
        // to free up UTXOs reserved by transactions that never confirmed.
        if let Some(bc) = self.block_context.as_ref() {
            lock(&self.pending)
                .reservations_mut()
                .evict_expired(chain_time(bc));
        }

        // A build reads `ctime` from the block time and witnesses against the
        // Dust root of this wallet's state, and the chain looks that root up
        // as `root_history.get(ctime)`.
        {
            let mut latest = ctx
                .latest_block_context
                .lock()
                .map_err(|_| WalletError::Sync("block context lock poisoned".into()))?;
            *latest = match (latest.take(), self.block_context.as_ref()) {
                (Some(chain), Some(own)) => Some(BlockContext {
                    tblock: own.tblock,
                    ..chain
                }),
                (chain, None) => chain,
                (None, own) => own.cloned(),
            };
        }

        // Populate UTXO state so the transaction builder can find our UTXOs.
        let unshielded = UnshieldedWallet::default(seed.clone());
        let owner = unshielded.user_address;
        let utxo_ctime = self
            .block_context
            .as_ref()
            .map(|bc| Timestamp::from_secs(bc.tblock.to_secs().saturating_sub(3600)))
            .unwrap_or_else(|| Timestamp::from_secs(0));

        // Filter out UTXOs reserved by recent (still-pending) builds so the
        // selector doesn't re-pick them before the indexer confirms the
        // spend.
        let pending_unshielded = self.reserved_unshielded_keys();

        {
            let mut guard = ctx
                .ledger_state
                .lock()
                .map_err(|_| WalletError::Sync("ledger state lock poisoned".into()))?;
            let mut ledger_state = (**guard).clone();

            let mut utxo_state = (*ledger_state.utxo).clone();
            for tracked in &self.unshielded_utxos {
                if pending_unshielded.contains(&tracked.spent_key()) {
                    continue;
                }
                let utxo = tracked_to_ledger_utxo(tracked, owner);
                utxo_state = utxo_state.insert(utxo, UtxoMeta { ctime: utxo_ctime });
            }
            ledger_state.utxo = Sp::new(utxo_state);
            *guard = Sp::new(ledger_state);
        }

        // The pending reservations go on clones only, so `self.dust_wallet`
        // keeps only the events that the indexer confirmed.
        {
            let mut shielded = ShieldedWallet::<DefaultDB>::default(seed.clone());
            shielded.state = self.zswap_state.clone();

            // Drop coins a recent still-pending build already spent so the
            // Zswap selector (`min_match_coin`) can't re-pick them before the
            // indexer confirms the spend. `remove` on an already-absent
            // nullifier (the spend confirmed and resync dropped the coin) is a
            // harmless no-op.
            for nullifier in self.reserved_shielded_nullifiers() {
                shielded.state.coins = shielded.state.coins.remove(&nullifier);
            }

            let mut dust = self.dust_wallet.clone();
            dust.dust_local_state = self.unreserved_dust().map(Sp::new);

            let wallet = ContextWallet {
                root_seed: Some(seed.clone()),
                shielded,
                unshielded: midnight_helpers::UnshieldedWallet::default(seed.clone()),
                dust,
            };

            ctx.wallets
                .lock()
                .map_err(|_| WalletError::Sync("wallets lock poisoned".into()))?
                .insert(seed, wallet);
        }

        Ok(())
    }

    // -------------------------------------------------------------------------
    // Accessors
    // -------------------------------------------------------------------------

    pub fn seed(&self) -> &Seed {
        &self.seed
    }

    /// See [`crate::Wallet::shielded_recipient`].
    pub fn shielded_recipient(&self) -> crate::ShieldedRecipient {
        crate::ShieldedRecipient {
            coin_public_key: self.secret_keys.coin_public_key().into_sdk(),
            enc_public_key: self.secret_keys.enc_public_key().into_sdk(),
        }
    }

    /// See [`crate::Wallet::network`].
    pub fn network(&self) -> &Network {
        &self.network
    }

    /// See [`crate::Wallet::unshielded_address`].
    pub fn unshielded_address(&self) -> String {
        self.unshielded_address.clone()
    }

    pub fn unshielded_utxos(&self) -> &[TrackedUtxo] {
        &self.unshielded_utxos
    }

    pub fn zswap_state(&self) -> &ZswapLocalState<DefaultDB> {
        &self.zswap_state
    }

    pub fn dust_wallet(&self) -> &DustWallet<DefaultDB> {
        &self.dust_wallet
    }

    pub fn block_context(&self) -> Option<&BlockContext> {
        self.block_context.as_ref()
    }

    /// Snapshot the inputs of a resync's replay phase. See [`ResyncPlan`]
    /// for the intended plan → run → commit flow.
    pub fn resync_plan(&self) -> ResyncPlan {
        ResyncPlan {
            secret_keys: self.secret_keys.clone(),
            unshielded_address: self.unshielded_address.clone(),
            dust_wallet: self.dust_wallet.clone(),
            dust_event_id: self.dust_event_id,
            zswap_state: self.zswap_state.clone(),
            zswap_event_id: self.zswap_event_id,
            unshielded_utxos: self.unshielded_utxos.clone(),
            last_tx_id: self.last_tx_id,
            last_block_height: self.last_block_height,
        }
    }

    /// See [`crate::wallet::WalletState::commit_resync`].
    pub fn commit_resync(&mut self, commit: ResyncCommit) -> Result<(), WalletError> {
        let ResyncCommit {
            dust_wallet,
            dust_event_id,
            last_dust_block_time,
            dust_nullifiers,
            zswap_state,
            zswap_event_id,
            unshielded_utxos,
            last_tx_id,
            last_block_height,
            spent_unshielded,
            chain_tblock,
            parameters,
        } = commit;

        // Dirty-check inputs, captured before the assignments below
        // overwrite them. Resync runs before every funding
        // (`MidnightWallet::fund_transaction`) and on user polling, so the
        // `save` at the end must be skipped when nothing durable moved;
        // otherwise every no-op resync rewrites the multi-MB
        // `zswap-N.bin`/`dust_wallet-N.bin` generation files. Dirty means:
        // a sync cursor advanced, the pending set changed across
        // `clear_confirmed`, or the chain's ledger parameters changed (a
        // governance move). `block_context` is recomputed on every resync
        // and is not persisted state, so it deliberately does not count.
        let cursors_advanced = dust_event_id != self.dust_event_id
            || zswap_event_id != self.zswap_event_id
            || Some(last_tx_id) != self.last_tx_id
            || last_block_height > self.last_block_height;
        let parameters_changed = parameters != self.parameters;

        self.dust_wallet = dust_wallet;
        self.dust_event_id = dust_event_id;
        self.zswap_state = zswap_state;
        self.zswap_event_id = zswap_event_id;
        self.unshielded_utxos = unshielded_utxos;
        self.last_tx_id = Some(last_tx_id);
        self.last_block_height = last_block_height;
        // Refresh parameters from the latest block so governance changes to
        // fees/TTL/dust rates take effect. Assigned before `global_ttl` is
        // read below so the anchor math uses the fresh value.
        self.parameters = parameters;

        let previous = self.block_context.as_ref().map(|bc| bc.tblock);
        let tblock = dust_anchor(
            last_dust_block_time,
            previous,
            chain_tblock,
            &self.parameters,
        );
        self.block_context = Some(midnight_helpers::make_block_context(
            tblock,
            Default::default(),
            chain_tblock,
        ));

        // Reservations whose spends this replay just observed confirmed are
        // no longer in flight; drop them so the underlying UTXOs become
        // spendable again immediately instead of waiting for TTL eviction.
        let pending_changed = lock(&self.pending)
            .reservations_mut()
            .clear_confirmed(&spent_unshielded, &dust_nullifiers);

        // Re-persist the committed state (moved cursors, refreshed
        // parameters, cleared pending set) so a crash before the next sync
        // resumes from here. Must run after `clear_confirmed` above: `save`
        // rewrites (or removes) `pending.json` from the in-memory set.
        // No-op resyncs skip this save (see the dirty-check above and
        // `persist`): pre-build resyncs are frequent and must not rewrite
        // the generation files.
        self.persist(cursors_advanced || parameters_changed || pending_changed)
    }

    // -------------------------------------------------------------------------
    // Coins the wallet owns but cannot discover
    // -------------------------------------------------------------------------

    /// See [`crate::Wallet::watch_for_coins`]. It persists once for all of
    /// `coins`.
    ///
    /// Registering nothing writes nothing, unless the last save failed.
    pub fn watch_for_coins(
        &mut self,
        coins: impl IntoIterator<Item = crate::CoinInfo>,
    ) -> Result<(), WalletError> {
        let coin_public_key = self.secret_keys.coin_public_key();
        let mut registered = false;
        for coin in coins {
            self.zswap_state = self
                .zswap_state
                .watch_for(&coin_public_key, &coin.into_ledger());
            registered = true;
        }
        self.persist(registered)
    }

    fn registered_coins(&self) -> Vec<midnight_helpers::CoinInfo> {
        self.zswap_state
            .pending_outputs
            .iter()
            .map(|(_commitment, coin)| *coin)
            .collect()
    }

    /// Every coin a state rebuilt from scratch must be told about before the
    /// replay: the registrations no replay has claimed yet, and the coins the
    /// wallet already holds.
    ///
    /// The held coins are the part that is easy to miss. A coin whose output
    /// carries no ciphertext this wallet can read is only ever claimed from a
    /// registration, and the ledger consumes that registration the moment a
    /// replay claims it. A replay seeded from the pending registrations alone
    /// would therefore meet that coin's output with nothing to claim it by,
    /// collapse its Merkle leaf, and lose a coin the wallet already had.
    /// Re-registering a coin that is discoverable through its ciphertext is
    /// harmless: both paths insert the same qualified coin under the same
    /// nullifier.
    ///
    /// A rebuilt state carries no `pending_spends`, and needs none: this
    /// crate never spends from `zswap_state` itself (a build spends from the
    /// [`LedgerContext`] copy) and records in-flight shielded spends in the
    /// pending reservation set instead.
    fn coins_to_register(&self) -> Vec<midnight_helpers::CoinInfo> {
        self.registered_coins()
            .into_iter()
            .chain(
                self.zswap_state
                    .coins
                    .iter()
                    .map(|(_nullifier, qualified)| (&*qualified).into()),
            )
            .collect()
    }

    /// Where this wallet persists its state, when it persists one.
    fn storage_dir(&self) -> Option<&Path> {
        self.storage_dir.as_deref()
    }

    /// Snapshot the inputs of a shielded rescan's replay phase. See
    /// [`ShieldedRescanPlan`] for the intended plan → run → commit flow.
    pub fn shielded_rescan_plan(&self) -> ShieldedRescanPlan {
        ShieldedRescanPlan {
            seed: self.seed.clone(),
            coins: self
                .coins_to_register()
                .into_iter()
                .map(IntoSdk::into_sdk)
                .collect(),
        }
    }

    /// See [`crate::wallet::WalletState::commit_shielded_rescan`].
    pub fn commit_shielded_rescan(
        &mut self,
        commit: ShieldedRescanCommit,
    ) -> Result<(), WalletError> {
        let ShieldedRescanCommit {
            zswap_state,
            zswap_event_id,
        } = commit;
        self.zswap_state = zswap_state;
        self.zswap_event_id = zswap_event_id;
        self.persist(true)
    }
}

// ---------------------------------------------------------------------------
// Replay helpers
// ---------------------------------------------------------------------------

/// The shielded state a zswap replay reached.
pub(crate) struct ZswapReplay {
    pub state: ZswapLocalState<DefaultDB>,
    /// The id of the last event applied.
    pub last_id: i64,
}

/// Replay the zswap stream of `seed`'s wallet from its first event, and claim
/// `coins` on the way.
///
/// The events before the hard fork to ledger 9 go through
/// [`crate::pre_fork`], which carries its state across the fork. The replay
/// goes on from the first ledger 9 event.
pub(crate) async fn replay_zswap_from_genesis(
    sub_client: &SubscriptionClient,
    seed: &Seed,
    coins: &[crate::CoinInfo],
    progress: Option<mpsc::Sender<SyncProgress>>,
) -> Result<ZswapReplay, WalletError> {
    let pre_fork = crate::pre_fork::replay(sub_client, seed, coins, progress.clone()).await?;
    let Some(fork_id) = pre_fork.fork_id else {
        return Ok(ZswapReplay {
            state: pre_fork.state,
            last_id: pre_fork.last_id,
        });
    };
    info!(fork_id, "the shielded stream continues in ledger 9");
    replay_zswap_events(
        sub_client,
        &zswap_keys(seed),
        pre_fork.state,
        fork_id,
        false,
        progress,
    )
    .await
}

/// Replay the ledger 9 events of the zswap stream from `start_id`.
pub(crate) async fn replay_zswap_events(
    sub_client: &SubscriptionClient,
    secret_keys: &SecretKeys,
    initial_state: ZswapLocalState<DefaultDB>,
    start_id: i64,
    resuming: bool,
    progress: Option<mpsc::Sender<SyncProgress>>,
) -> Result<ZswapReplay, WalletError> {
    let mut state = initial_state;
    let replay = replay_ledger_events(
        sub_client,
        LedgerStream::Zswap,
        Generation::V9,
        start_id,
        resuming,
        progress,
        |id, raw, ledger| {
            if ledger != Generation::V9 {
                return Err(WalletError::Sync(format!(
                    "zswap event id={id} is ledger 8 data after the fork"
                )));
            }
            let ev = decode_event(raw, "zswap")?;
            state = state
                .replay_events(secret_keys, [&ev])
                .map_err(|e| WalletError::Sync(format!("replay zswap event id={id}: {e}")))?;
            Ok(())
        },
    )
    .await?;

    Ok(ZswapReplay {
        state,
        last_id: replay.last_id,
    })
}

/// The Dust state a dust replay reached.
pub(crate) struct DustReplay {
    pub wallet: DustWallet<DefaultDB>,
    /// The id of the last event consumed.
    pub last_id: i64,
    /// The block time of the last event that carried one.
    pub last_block_time: Option<Timestamp>,
    /// The nullifiers of the Dust spends the replay saw processed.
    pub spend_nullifiers: Vec<DustNullifier>,
}

/// Replay the Dust event stream from `start_id` into `dust_wallet`.
///
/// The replay consumes the ledger 8 events without applying them. The hard
/// fork to ledger 9 replaced the chain's Dust state with an empty one, so
/// these events describe Dust that no longer exists.
pub(crate) async fn replay_dust_events(
    sub_client: &SubscriptionClient,
    mut dust_wallet: DustWallet<DefaultDB>,
    start_id: i64,
    resuming: bool,
    checkpoint: Option<impl Fn(&DustWallet<DefaultDB>, i64)>,
    progress: Option<mpsc::Sender<SyncProgress>>,
) -> Result<DustReplay, WalletError> {
    let mut last_block_time: Option<Timestamp> = None;
    // Nullifiers of every DustSpendProcessed event seen during this replay,
    // surfaced to the caller so it can clear confirmed pending reservations.
    let mut spend_nullifiers: Vec<DustNullifier> = Vec::new();
    let mut consumed: u64 = 0;
    // The closure owns `checkpoint` and borrows the rest through these
    // bindings. The sync's checkpoint is `Send` but not `Sync`, so a borrow of
    // it would make the replay future lose `Send`.
    let (wallet, last_time, nullifiers) = (
        &mut dust_wallet,
        &mut last_block_time,
        &mut spend_nullifiers,
    );
    let replay = replay_ledger_events(
        sub_client,
        LedgerStream::Dust,
        Generation::V9,
        start_id,
        resuming,
        progress,
        move |id, raw, ledger| {
            if ledger == Generation::V9 {
                let ev = decode_event(raw, "dust")?;
                wallet
                    .replay_events([&ev])
                    .map_err(|e| WalletError::Sync(format!("apply dust event id={id}: {e}")))?;
                if let Some(t) = event_block_time(&ev) {
                    *last_time = Some(t);
                }
                if let Some(n) = event_spend_nullifier(&ev) {
                    nullifiers.push(n);
                }
            }
            consumed += 1;
            if consumed.is_multiple_of(DUST_CHECKPOINT_INTERVAL)
                && let Some(save) = &checkpoint
            {
                save(wallet, id);
            }
            Ok(())
        },
    )
    .await?;

    Ok(DustReplay {
        wallet: dust_wallet,
        last_id: replay.last_id,
        last_block_time,
        spend_nullifiers,
    })
}

/// Extract the block_time from a dust event, if present.
fn event_block_time(event: &Event<DefaultDB>) -> Option<Timestamp> {
    match &event.content {
        EventDetails::DustInitialUtxo { block_time, .. } => Some(*block_time),
        EventDetails::DustSpendProcessed { block_time, .. } => Some(*block_time),
        EventDetails::DustGenerationDtimeUpdate { block_time, .. } => Some(*block_time),
        _ => None,
    }
}

/// Extract the spend nullifier from a dust event, if it is a processed
/// spend. Used to clear matching `PendingReservations` dust batches once
/// the chain confirms them.
fn event_spend_nullifier(event: &Event<DefaultDB>) -> Option<DustNullifier> {
    match &event.content {
        EventDetails::DustSpendProcessed { nullifier, .. } => Some(*nullifier),
        _ => None,
    }
}

fn tracked_to_ledger_utxo(
    tracked: &TrackedUtxo,
    owner: midnight_helpers::UserAddress,
) -> LedgerUtxo {
    LedgerUtxo {
        value: tracked.value,
        owner,
        type_: tracked.token_type.into_ledger(),
        intent_hash: midnight_helpers::IntentHash(tracked.intent_hash),
        output_no: tracked.output_index,
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use midnight_helpers::midnight_serialize::tagged_serialize;
    use midnight_helpers::mn_ledger::events::{EventSource, ZswapPreimageEvidence};
    use midnight_helpers::{HashOutput, Recipient, TransactionHash};

    use super::*;

    /// An event with an all-zero source, for tests that read only its content.
    pub(super) fn zero_source_event(content: EventDetails<DefaultDB>) -> Event<DefaultDB> {
        Event {
            source: EventSource {
                transaction_hash: TransactionHash(HashOutput([0u8; 32])),
                logical_segment: 0,
                physical_segment: 0,
            },
            content,
        }
    }

    /// The hex of a ledger 9 event for an output of `coin` to `seed`'s
    /// wallet that carries no discovery ciphertext. The commitment lands in
    /// the Merkle tree and nothing else identifies the coin, so only a
    /// registration claims it.
    pub(crate) fn ciphertextless_output(
        seed: &Seed,
        coin: &crate::CoinInfo,
        mt_index: u64,
    ) -> String {
        let commitment = coin
            .into_ledger()
            .commitment(&Recipient::User(zswap_keys(seed).coin_public_key()));
        let event = zero_source_event(EventDetails::ZswapOutput {
            commitment,
            preimage_evidence: ZswapPreimageEvidence::None,
            contract: None,
            mt_index,
        });
        let mut raw = Vec::new();
        tagged_serialize(&event, &mut raw).unwrap();
        hex::encode(raw)
    }
}

#[cfg(test)]
mod tests {
    use midnight_helpers::coin_structure::coin::Commitment;
    use midnight_helpers::midnight_serialize::tagged_serialize;
    use midnight_helpers::mn_ledger::dust::DustCommitment;
    use midnight_helpers::{
        DustNullifier, Fr, HashOutput, INITIAL_PARAMETERS, Nonce, Nullifier, QualifiedInfo,
        Recipient, ShieldedTokenType,
    };

    use midnight_types::build::{ReservedInput, SpentInputs};

    use super::super::pending::test_support::{dust_batch, ukey};
    use super::test_support::{ciphertextless_output, zero_source_event};
    use super::*;
    use crate::wallet::test_support::{test_seed, unreachable_coin};

    /// The anchor sets the `ctime` that the node checks, so a wrong one fails
    /// no sync. Only the node rejects the transaction later.
    #[test]
    fn dust_anchor_keeps_the_newest_race_safe_time_inside_the_dust_window() {
        const HOUR: u64 = 60 * 60;
        let parameters = params_with(|p| {
            p.global_ttl = midnight_helpers::Duration::from_secs(14 * 24 * 60 * 60);
            p.dust.dust_grace_period = midnight_helpers::Duration::from_secs(3 * 60 * 60);
        });
        let now = 100 * HOUR;
        let at = Timestamp::from_secs;
        let cases = [
            // The last Dust event wins over an older anchor.
            (
                Some(at(now - HOUR)),
                Some(at(now - 2 * HOUR)),
                at(now - HOUR + 1),
            ),
            // A replay that saw no Dust event keeps the previous anchor.
            (None, Some(at(now - HOUR)), at(now - HOUR)),
            // Inside `global_ttl` but older than the Dust grace period.
            (Some(at(now - 4 * HOUR)), None, at(now)),
        ];
        for (last_dust, previous, expected) in cases {
            assert_eq!(
                dust_anchor(last_dust, previous, at(now), &parameters),
                expected,
                "last Dust event {last_dust:?}, previous anchor {previous:?}"
            );
        }
    }

    #[test]
    fn event_spend_nullifier_matches_dust_spend_processed_only() {
        let nullifier = DustNullifier(Fr::from(7u64));
        let spend = zero_source_event(EventDetails::DustSpendProcessed {
            commitment: DustCommitment(Fr::from(8u64)),
            commitment_index: 0,
            nullifier,
            v_fee: 1,
            declared_time: Timestamp::from_secs(0),
            block_time: Timestamp::from_secs(0),
        });
        assert_eq!(event_spend_nullifier(&spend), Some(nullifier));

        let other = zero_source_event(EventDetails::ZswapInput {
            nullifier: Nullifier(HashOutput([1u8; 32])),
            contract: None,
        });
        assert_eq!(event_spend_nullifier(&other), None);
    }

    /// Minimal offline wallet for unit tests: fresh state, no sync.
    fn test_wallet(storage_dir: Option<PathBuf>) -> Wallet {
        let seed = test_seed();
        let shielded = ShieldedWallet::<DefaultDB>::default((&seed).into_ledger());
        let secret_keys = shielded.secret_keys().clone();
        Wallet {
            seed: seed.clone(),
            secret_keys,
            network: Network::Undeployed,
            unshielded_address: "mn_addr_undeployed1test".into(),
            zswap_state: shielded.state.clone(),
            zswap_event_id: 0,
            dust_wallet: DustWallet::default((&seed).into_ledger(), Some(&INITIAL_PARAMETERS)),
            dust_event_id: 0,
            unshielded_utxos: Vec::new(),
            last_block_height: 0,
            last_tx_id: None,
            chain_pin: None,
            parameters: INITIAL_PARAMETERS,
            block_context: None,
            pending: Arc::new(Mutex::new(PendingBook::new(
                PendingReservations::default(),
                storage_dir.clone().map(|dir| PendingFile {
                    dir,
                    network: Network::Undeployed.as_str().to_owned(),
                    wallet_id: wallet_storage_id("mn_addr_undeployed1test"),
                }),
            ))),
            storage_dir,
            indexer_url: "http://indexer.invalid".into(),
        }
    }

    /// A context from a node, as a provider fills one, with no block context.
    fn chain_context() -> Arc<LedgerContext<DefaultDB>> {
        midnight_types::build::execution_context(Network::Undeployed, INITIAL_PARAMETERS, None)
            .expect("execution context")
    }

    fn block_with_params(ledger_parameters: Option<String>) -> midnight_indexer_client::Block {
        midnight_indexer_client::Block {
            hash: "00".repeat(32),
            height: 1,
            protocol_version: None,
            timestamp: Some(1_000),
            author: None,
            ledger_parameters,
        }
    }

    #[test]
    fn decode_ledger_parameters_round_trips_block_parameters() {
        let mut encoded = Vec::new();
        tagged_serialize(&INITIAL_PARAMETERS, &mut encoded).unwrap();
        let block = block_with_params(Some(hex::encode(&encoded)));

        let decoded = decode_ledger_parameters(&block).unwrap();

        let mut reencoded = Vec::new();
        tagged_serialize(&decoded, &mut reencoded).unwrap();
        assert_eq!(reencoded, encoded);
    }

    #[test]
    fn decode_ledger_parameters_rejects_missing_or_malformed() {
        assert!(matches!(
            decode_ledger_parameters(&block_with_params(None)),
            Err(WalletError::Sync(_))
        ));
        assert!(matches!(
            decode_ledger_parameters(&block_with_params(Some("zz".into()))),
            Err(WalletError::Sync(_))
        ));
    }

    /// A build evicts an expired reservation before it selects, so the
    /// balance read must not count one as reserved either. Expiry is in chain
    /// time, which runs ahead of the Dust anchor: a reservation that expired
    /// between the two is dead, and one that expires at the chain time is not
    /// yet.
    #[test]
    fn reserved_dust_nullifiers_skip_an_expired_reservation() {
        let mut wallet = test_wallet(None);
        let anchor = Timestamp::from_secs(100_000);
        let chain_now = anchor + midnight_helpers::Duration::from_secs(600);
        wallet.block_context = Some(midnight_helpers::make_block_context(
            anchor,
            Default::default(),
            chain_now,
        ));
        let between = anchor + midnight_helpers::Duration::from_secs(300);
        wallet.reserve_pending(
            vec![dust_batch(&[1])],
            Vec::new(),
            Vec::new(),
            anchor,
            between,
        );
        wallet.reserve_pending(
            vec![dust_batch(&[2])],
            Vec::new(),
            Vec::new(),
            anchor,
            chain_now,
        );

        assert_eq!(
            wallet.reserved_dust_nullifiers(),
            vec![DustNullifier(Fr::from(2u64))]
        );
    }

    /// The shielded and unshielded readings hold the inputs of a live
    /// reservation and free those of an expired one, as a build does.
    #[test]
    fn readings_hold_only_the_inputs_of_a_live_reservation() {
        let mut wallet = test_wallet(None);
        let now = Timestamp::from_secs(100_000);
        wallet.block_context = Some(midnight_helpers::make_block_context(
            now,
            Default::default(),
            now,
        ));
        let reserved_at = now - midnight_helpers::Duration::from_secs(60);
        let expired = now - midnight_helpers::Duration::from_secs(1);
        let expired_coin = insert_shielded_coin(&mut wallet, 1, 100);
        let live_coin = insert_shielded_coin(&mut wallet, 2, 200);
        let expired_utxo = tracked_night_utxo();
        let live_utxo = TrackedUtxo {
            output_index: 1,
            value: 58,
            ..tracked_night_utxo()
        };
        wallet.unshielded_utxos = vec![expired_utxo.clone(), live_utxo.clone()];
        wallet.reserve_pending(
            Vec::new(),
            vec![expired_utxo.spent_key()],
            vec![expired_coin],
            reserved_at,
            expired,
        );
        wallet.reserve_pending(
            Vec::new(),
            vec![live_utxo.spent_key()],
            vec![live_coin],
            reserved_at,
            now,
        );

        let spendable: Vec<u128> = wallet
            .spendable_shielded_coins()
            .iter()
            .map(|coin| coin.value)
            .collect();
        assert_eq!(spendable, vec![100]);
        let balance = wallet.balance();
        let token = midnight_types::ShieldedTokenType(midnight_types::HashOutput([0; 32]));
        assert_eq!(balance.shielded_value(token), 300);
        assert_eq!(balance.spendable_shielded_value(token), 100);
        assert_eq!(balance.unshielded_value(midnight_types::NIGHT), 100);
        assert_eq!(
            balance.spendable_unshielded_value(midnight_types::NIGHT),
            42
        );
    }

    /// One shielded coin in the wallet's Zswap state, keyed by `nullifier`. The
    /// key is what selection and the reservation filter compare against, so it
    /// need not be a cryptographically-derived nullifier for this test.
    fn insert_shielded_coin(wallet: &mut Wallet, nullifier_byte: u8, value: u128) -> Nullifier {
        let nullifier = Nullifier(HashOutput([nullifier_byte; 32]));
        let coin = QualifiedInfo {
            nonce: Nonce(HashOutput([nullifier_byte; 32])),
            type_: ShieldedTokenType(HashOutput([0u8; 32])),
            value,
            mt_index: 0,
        };
        wallet.zswap_state.coins = wallet.zswap_state.coins.insert(nullifier, coin);
        nullifier
    }

    /// A shielded coin reserved by a pending build is hidden from both
    /// `spendable_shielded_coins` and the Zswap coin set the build context hands
    /// the selector, so a later in-process build cannot re-select it.
    #[test]
    fn reserved_shielded_coin_is_filtered_from_selection_and_context() {
        let mut wallet = test_wallet(None);
        let nullifier = insert_shielded_coin(&mut wallet, 7, 100);

        // Visible before it is reserved.
        assert_eq!(wallet.spendable_shielded_coins().len(), 1);

        // Reserving it removes it from selection.
        wallet.reserve_pending(
            Vec::new(),
            Vec::new(),
            vec![nullifier],
            Timestamp::from_secs(100),
            Timestamp::MAX,
        );
        assert!(wallet.spendable_shielded_coins().is_empty());

        // And it is gone from the coin set the funding view exposes (this is
        // the `coins.remove(nullifier)` path in add_funding).
        let ctx = chain_context();
        wallet.add_funding(&ctx).expect("add funding");
        let wallets = ctx.wallets.lock().expect("wallets lock");
        let ctx_wallet = wallets
            .get(&wallet.seed().into_ledger())
            .expect("funding wallet in context");
        assert_eq!(
            ctx_wallet.shielded.state.coins.iter().count(),
            0,
            "reserved coin must be removed from the build context's Zswap state"
        );
    }

    /// A NIGHT UTXO that `ukey(0xab, 0)` names.
    fn tracked_night_utxo() -> TrackedUtxo {
        TrackedUtxo {
            owner: "mn_addr_undeployed1test".into(),
            token_type: midnight_types::NIGHT,
            value: 42,
            intent_hash: midnight_types::HashOutput([0xab; 32]),
            output_index: 0,
            ctime: Some(0),
            registered_for_dust_generation: Some(false),
        }
    }

    #[test]
    fn add_funding_puts_the_wallet_and_its_utxos_in() {
        let mut wallet = test_wallet(None);
        wallet.unshielded_utxos = vec![tracked_night_utxo()];

        let ctx = chain_context();
        wallet.add_funding(&ctx).expect("add funding");

        assert!(
            ctx.wallets
                .lock()
                .expect("wallets lock")
                .contains_key(&wallet.seed().into_ledger()),
            "funding must insert this wallet"
        );
        assert_eq!(
            ctx.with_ledger_state(|s| s.utxo.utxos.iter().count()),
            1,
            "funding must insert the wallet's unspent UTXOs"
        );
    }

    #[test]
    fn add_funding_skips_a_reserved_utxo() {
        let mut wallet = test_wallet(None);
        wallet.unshielded_utxos = vec![tracked_night_utxo()];
        wallet.reserve_pending(
            Vec::new(),
            vec![ukey(0xab, 0)],
            Vec::new(),
            Timestamp::from_secs(100),
            Timestamp::MAX,
        );

        let ctx = chain_context();
        wallet.add_funding(&ctx).expect("add funding");

        assert_eq!(
            ctx.with_ledger_state(|s| s.utxo.utxos.iter().count()),
            0,
            "a UTXO a pending build reserved must not reach the funding view"
        );
    }

    #[test]
    fn add_funding_refuses_a_context_it_already_funds() {
        let mut wallet = test_wallet(None);
        let ctx = chain_context();
        wallet.add_funding(&ctx).expect("first funding");

        // A second pass would merge into the first one's UTXO set, leaving an
        // input a pending build reserved still selectable.
        assert!(
            wallet.add_funding(&ctx).is_err(),
            "funding the same context twice must be refused"
        );
    }

    /// The Dust spends witness a root that the chain looks up at the block
    /// time of the context, and the TTL counts from its chain time. A funding
    /// view that kept the fill's block time fails at the chain, and one that
    /// took the wallet's whole block context counts the TTL from the wallet's
    /// older view.
    #[test]
    fn add_funding_takes_the_dust_anchor_and_keeps_the_chain_time() {
        let mut wallet = test_wallet(None);
        let anchor = Timestamp::from_secs(1_000);
        wallet.block_context = Some(midnight_helpers::make_block_context(
            anchor,
            Default::default(),
            Timestamp::from_secs(1_100),
        ));
        let node_time = Timestamp::from_secs(1_200);
        let ctx = midnight_types::build::execution_context(
            Network::Undeployed,
            INITIAL_PARAMETERS,
            Some(midnight_helpers::make_block_context(
                node_time,
                Default::default(),
                node_time,
            )),
        )
        .expect("execution context");

        wallet.add_funding(&ctx).expect("add funding");

        let funded = ctx.latest_block_context();
        assert_eq!(funded.tblock, anchor);
        assert_eq!(chain_time(&funded), node_time);
    }

    /// `sign_transaction` marks a reservation proven, and only a proven one
    /// reaches `pending.json`. A mark that stayed in memory would lose the
    /// reservation of a proven transaction at a restart.
    #[test]
    fn a_reservation_marked_proven_reaches_pending_json() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut wallet = test_wallet(Some(dir.path().to_path_buf()));
        wallet.reserve_pending(
            Vec::new(),
            vec![ukey(0xab, 0)],
            Vec::new(),
            Timestamp::from_secs(100),
            Timestamp::MAX,
        );

        lock(&wallet.pending).mark_proven(&spent_utxo(ukey(0xab, 0), 100));

        let loaded = PendingReservations::load(dir.path(), "undeployed", &wallet.storage_id())
            .unwrap()
            .expect("pending.json should exist after mark_proven");
        assert_eq!(loaded.unshielded_spends().count(), 1);
    }

    /// What a build that reserved only the UTXO `key` at `reserved_at_secs`
    /// reports spending.
    fn spent_utxo(key: SpentUtxoKey, reserved_at_secs: u64) -> SpentInputs {
        SpentInputs {
            unshielded: vec![ReservedInput {
                input: key,
                reserved_at: Timestamp::from_secs(reserved_at_secs),
            }],
            ..Default::default()
        }
    }

    /// The commitment `coin`'s output carries when `wallet` owns it.
    fn commitment_of(wallet: &Wallet, coin: &crate::CoinInfo) -> Commitment {
        coin.into_ledger()
            .commitment(&Recipient::User(wallet.secret_keys.coin_public_key()))
    }

    #[test]
    fn watch_for_coins_records_the_commitment_the_output_carries() {
        let mut wallet = test_wallet(None);
        let coin = unreachable_coin(9, 42);

        wallet.watch_for_coins([coin]).unwrap();

        assert!(
            wallet
                .zswap_state()
                .pending_outputs
                .contains_key(&commitment_of(&wallet, &coin)),
            "registration must key on the commitment a replay meets on chain"
        );
    }

    /// Registering nothing must not rewrite the multi-MB generation files.
    #[test]
    fn registering_nothing_writes_nothing() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut wallet = test_wallet(Some(dir.path().to_path_buf()));
        wallet.save(dir.path()).unwrap();

        wallet.watch_for_coins([]).unwrap();

        assert_eq!(stored_generations(dir.path()), vec![1]);
    }

    /// A coin registered before it lands on chain has to survive a restart,
    /// so the registration is part of the persisted wallet state.
    #[test]
    fn watch_for_coins_persists_the_registration() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut wallet = test_wallet(Some(dir.path().to_path_buf()));
        let coin = unreachable_coin(9, 42);

        wallet.watch_for_coins([coin]).unwrap();

        let loaded = super::super::snapshot::load(dir.path(), "undeployed", &wallet.storage_id())
            .unwrap()
            .expect("registration must be saved");
        assert!(
            loaded
                .zswap_state
                .pending_outputs
                .contains_key(&commitment_of(&wallet, &coin))
        );
    }

    /// The plan replays from nothing, so it must carry every coin the replay
    /// claims by registration: the registrations no replay has claimed, and
    /// the coins the wallet holds, whose own registrations a replay consumed.
    #[test]
    fn shielded_rescan_plan_carries_the_registrations_and_the_held_coins() {
        let mut wallet = test_wallet(None);
        insert_shielded_coin(&mut wallet, 7, 100);
        let coin = unreachable_coin(9, 42);
        wallet.watch_for_coins([coin]).unwrap();

        let plan = wallet.shielded_rescan_plan();

        assert!(plan.coins.contains(&coin), "the registration rides along");
        assert!(
            plan.coins.iter().any(|held| held.value == 100),
            "the held coin rides along"
        );
    }

    #[test]
    fn commit_shielded_rescan_replaces_the_shielded_state_and_cursor() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut wallet = test_wallet(Some(dir.path().to_path_buf()));
        insert_shielded_coin(&mut wallet, 7, 100);
        wallet.zswap_event_id = 4;
        wallet.dust_event_id = 11;

        wallet
            .commit_shielded_rescan(ShieldedRescanCommit {
                zswap_state: ZswapLocalState::new(),
                zswap_event_id: 9,
            })
            .unwrap();

        assert!(wallet.spendable_shielded_coins().is_empty());
        assert_eq!(wallet.zswap_event_id, 9);
        assert_eq!(wallet.dust_event_id, 11, "a rescan is shielded-only");
        assert_eq!(stored_generations(dir.path()), vec![1]);
    }

    /// A rescan's plan seeds the replay with every registration, so one
    /// missing from the state it commits was consumed: the replay claimed the
    /// coin, and here also saw it spent. Putting the registration back would
    /// make every later replay watch for a coin that is gone.
    #[test]
    fn commit_shielded_rescan_keeps_no_registration_its_replay_consumed() {
        let mut wallet = test_wallet(None);
        wallet.watch_for_coins([unreachable_coin(9, 42)]).unwrap();

        wallet
            .commit_shielded_rescan(ShieldedRescanCommit {
                zswap_state: ZswapLocalState::new(),
                zswap_event_id: 9,
            })
            .unwrap();

        assert!(wallet.registered_coins().is_empty());
    }

    /// Storage generations present on disk, identified by the `zswap-N.bin`
    /// files under `base` (recursively, since the per-wallet directory name
    /// is a seed digest). A no-op resync must leave this unchanged; a dirty
    /// one bumps it.
    fn stored_generations(base: &Path) -> Vec<u64> {
        fn walk(dir: &Path, out: &mut Vec<u64>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(&path, out);
                } else if let Some(generation) = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .and_then(|n| n.strip_prefix("zswap-"))
                    .and_then(|n| n.strip_suffix(".bin"))
                    .and_then(|n| n.parse().ok())
                {
                    out.push(generation);
                }
            }
        }
        let mut out = Vec::new();
        walk(base, &mut out);
        out.sort_unstable();
        out
    }

    /// A `ResyncCommit` carrying exactly the wallet's current durable state:
    /// the shape of a resync that found nothing new on chain.
    fn noop_commit(wallet: &Wallet) -> ResyncCommit {
        ResyncCommit {
            dust_wallet: wallet.dust_wallet.clone(),
            dust_event_id: wallet.dust_event_id,
            last_dust_block_time: None,
            dust_nullifiers: Vec::new(),
            zswap_state: wallet.zswap_state.clone(),
            zswap_event_id: wallet.zswap_event_id,
            unshielded_utxos: wallet.unshielded_utxos.clone(),
            last_tx_id: wallet.last_tx_id.unwrap_or(0),
            last_block_height: wallet.last_block_height,
            spent_unshielded: Vec::new(),
            chain_tblock: Timestamp::from_secs(1_000),
            parameters: wallet.parameters.clone(),
        }
    }

    /// A wallet that has saved once, as generation 1, to the returned
    /// directory. Its tx cursor is `Some`, so that [`noop_commit`] mirrors
    /// it: with no cursor, every commit counts as a cursor move.
    fn saved_wallet() -> (tempfile::TempDir, Wallet) {
        let dir = tempfile::TempDir::new().unwrap();
        let mut wallet = test_wallet(Some(dir.path().to_path_buf()));
        wallet.last_tx_id = Some(3);
        wallet.save(dir.path()).unwrap();
        (dir, wallet)
    }

    /// A resync that keeps the Dust anchor must still move the chain time
    /// that a build counts its TTL from. A TTL from the anchor reaches the
    /// chain already expired when it is shorter than the anchor's lag.
    #[test]
    fn a_resync_keeps_the_chain_time_beside_the_dust_anchor() {
        let mut wallet = test_wallet(None);
        let anchor = Timestamp::from_secs(1_000);
        wallet.block_context = Some(midnight_helpers::make_block_context(
            anchor,
            Default::default(),
            anchor,
        ));
        let mut commit = noop_commit(&wallet);
        commit.chain_tblock = anchor + midnight_helpers::Duration::from_secs(600);
        wallet.commit_resync(commit).unwrap();

        let context = wallet.block_context().expect("a block context");
        assert_eq!(context.tblock, anchor, "no Dust event moved the anchor");
        assert_eq!(
            chain_time(context),
            anchor + midnight_helpers::Duration::from_secs(600)
        );
    }

    #[test]
    fn noop_resync_commit_skips_persistence() {
        // Seam for the resync commit path: resync runs before every build,
        // so a commit that changes no durable state must not rewrite the
        // generation files, even though it refreshes `block_context`.
        let (dir, mut wallet) = saved_wallet();
        assert_eq!(stored_generations(dir.path()), vec![1]);

        let commit = noop_commit(&wallet);
        wallet.commit_resync(commit).unwrap();

        assert_eq!(stored_generations(dir.path()), vec![1]);
        // The non-durable block context was still refreshed.
        assert!(wallet.block_context.is_some());
    }

    #[test]
    fn resync_commit_persists_when_cursor_advances() {
        let (dir, mut wallet) = saved_wallet();

        let mut commit = noop_commit(&wallet);
        commit.dust_event_id += 1;
        wallet.commit_resync(commit).unwrap();

        assert_eq!(wallet.dust_event_id, 1);
        assert_eq!(stored_generations(dir.path()), vec![2]);
    }

    #[test]
    fn resync_commit_persists_when_parameters_change() {
        let (dir, mut wallet) = saved_wallet();

        let mut commit = noop_commit(&wallet);
        commit
            .parameters
            .cardano_to_midnight_bridge_fee_basis_points += 1;
        wallet.commit_resync(commit).unwrap();

        assert_eq!(stored_generations(dir.path()), vec![2]);
    }

    #[test]
    fn resync_commit_persists_when_reservation_cleared() {
        let (dir, mut wallet) = saved_wallet();
        let key = ukey(0xab, 0);
        wallet.reserve_pending(
            Vec::new(),
            vec![key.clone()],
            Vec::new(),
            Timestamp::from_secs(100),
            Timestamp::MAX,
        );
        lock(&wallet.pending).mark_proven(&spent_utxo(key.clone(), 100));

        let mut commit = noop_commit(&wallet);
        commit.spent_unshielded = vec![key];
        wallet.commit_resync(commit).unwrap();

        assert!(lock(&wallet.pending).reservations().is_empty());
        assert_eq!(stored_generations(dir.path()), vec![2]);
        assert!(
            PendingReservations::load(dir.path(), "undeployed", &wallet.storage_id())
                .unwrap()
                .is_none()
        );
    }

    /// A failed save leaves the memory ahead of the disk. The next commit
    /// must save even when it moves nothing, and then skip no-op saves again.
    #[test]
    fn resync_commit_retries_a_failed_save() {
        // A regular file at `storage_dir` fails every save until it goes.
        let dir = tempfile::TempDir::new().unwrap();
        let base = dir.path().join("not-a-directory");
        std::fs::write(&base, b"occupied").unwrap();
        let mut wallet = test_wallet(Some(base.clone()));
        wallet.last_tx_id = Some(3);

        let mut commit = noop_commit(&wallet);
        commit.dust_event_id += 1;
        assert!(matches!(
            wallet.commit_resync(commit),
            Err(WalletError::Storage(_))
        ));

        std::fs::remove_file(&base).unwrap();
        wallet.commit_resync(noop_commit(&wallet)).unwrap();

        let loaded = super::super::snapshot::load(&base, "undeployed", &wallet.storage_id())
            .unwrap()
            .expect("a no-op commit after a failed save must save");
        assert_eq!(loaded.dust_event_id, 1);

        wallet.commit_resync(noop_commit(&wallet)).unwrap();
        assert_eq!(
            stored_generations(&base),
            vec![1],
            "a successful save must stop the no-op commits from saving"
        );
    }

    #[test]
    fn commit_resync_clears_confirmed_against_commit_time_pending() {
        // The plan → run → commit split releases the wallet lock during the
        // replay, so a transfer build can reserve pending entries between
        // the plan snapshot and the commit. The commit must merge with the
        // commit-time pending set: clear exactly what the replay observed
        // confirmed, preserve reservations made after the snapshot.
        let mut wallet = test_wallet(None);
        wallet.last_tx_id = Some(3);
        let confirmed = ukey(0xaa, 0);
        wallet.reserve_pending(
            Vec::new(),
            vec![confirmed.clone()],
            Vec::new(),
            Timestamp::from_secs(100),
            Timestamp::MAX,
        );

        // Snapshot the replay inputs (the plan itself must not mutate).
        let _plan = wallet.resync_plan();

        // A transfer build interleaves while the (conceptual) replay runs.
        let late = ukey(0xbb, 1);
        wallet.reserve_pending(
            Vec::new(),
            vec![late.clone()],
            Vec::new(),
            Timestamp::from_secs(101),
            Timestamp::MAX,
        );

        // The replay observed only the first reservation's spend.
        let mut commit = noop_commit(&wallet);
        commit.spent_unshielded = vec![confirmed];
        wallet.commit_resync(commit).unwrap();

        let remaining: Vec<_> = lock(&wallet.pending)
            .reservations()
            .unshielded_spends()
            .map(|spend| spend.key.clone())
            .collect();
        assert_eq!(remaining, vec![late], "late reservation must survive");
    }

    #[test]
    fn a_proven_mark_keeps_the_reservation_when_persistence_fails() {
        // `storage_dir` points at a regular file, so `save_pending` cannot
        // create the wallet directory and the disk write fails. The write
        // is best-effort: no panic, and the in-memory reservation stands.
        let dir = tempfile::TempDir::new().unwrap();
        let blocker = dir.path().join("not-a-directory");
        std::fs::write(&blocker, b"occupied").unwrap();

        let mut wallet = test_wallet(Some(blocker.clone()));
        wallet.last_tx_id = Some(3);
        wallet.reserve_pending(
            Vec::new(),
            vec![ukey(0xab, 0)],
            Vec::new(),
            Timestamp::from_secs(100),
            Timestamp::MAX,
        );
        lock(&wallet.pending).mark_proven(&spent_utxo(ukey(0xab, 0), 100));

        assert_eq!(
            lock(&wallet.pending)
                .reservations()
                .unshielded_spends()
                .count(),
            1
        );

        // Once the disk works again, a commit that moves nothing must
        // still write the reservation.
        std::fs::remove_file(&blocker).unwrap();
        wallet.commit_resync(noop_commit(&wallet)).unwrap();
        let saved = PendingReservations::load(&blocker, "undeployed", &wallet.storage_id())
            .unwrap()
            .expect("a no-op commit after a failed pending write must save");
        assert_eq!(saved.unshielded_spends().count(), 1);
    }

    fn params_with(
        mutate: impl FnOnce(&mut midnight_helpers::LedgerParameters),
    ) -> midnight_helpers::LedgerParameters {
        let mut p = INITIAL_PARAMETERS;
        mutate(&mut p);
        p
    }

    #[test]
    fn validate_ledger_parameters_rejects_zeroed_fields() {
        use midnight_helpers::base_crypto::cost_model::FixedPoint;

        let cases = vec![
            (
                "global_ttl",
                params_with(|p| p.global_ttl = midnight_helpers::Duration::from_secs(0)),
            ),
            (
                "dust.night_dust_ratio",
                params_with(|p| p.dust.night_dust_ratio = 0),
            ),
            (
                "dust.generation_decay_rate",
                params_with(|p| p.dust.generation_decay_rate = 0),
            ),
            (
                "fee_prices.overall_price",
                params_with(|p| p.fee_prices.overall_price = FixedPoint::ZERO),
            ),
        ];
        for (expected_field, params) in cases {
            match validate_ledger_parameters(&params) {
                Err(WalletError::CorruptParameters { field, .. }) => {
                    assert_eq!(field, expected_field);
                }
                other => panic!("expected CorruptParameters for {expected_field}, got {other:?}"),
            }
        }
    }

    #[test]
    fn decode_ledger_parameters_rejects_corrupt_values_at_decode() {
        // A structurally valid blob with a zeroed TTL must be rejected by
        // `decode_ledger_parameters` itself, so both the initial-sync and
        // the resync plan/run paths refuse it before any fee math runs.
        let corrupt = params_with(|p| p.global_ttl = midnight_helpers::Duration::from_secs(0));
        let mut encoded = Vec::new();
        tagged_serialize(&corrupt, &mut encoded).unwrap();

        let err = decode_ledger_parameters(&block_with_params(Some(hex::encode(&encoded))))
            .expect_err("zeroed global_ttl must be rejected at decode");
        assert!(
            matches!(
                err,
                WalletError::CorruptParameters {
                    field: "global_ttl",
                    ..
                }
            ),
            "got: {err:?}"
        );
    }

    /// Mock-WebSocket-server test for recovering a coin whose output carries
    /// no ciphertext this wallet can read, driven through the shielded
    /// rescan. The mock server lives in `midnight_indexer_client::testutil`.
    mod shielded_rescan_ws {
        use midnight_indexer_client::testutil::{
            accept_subscriber, bind, ledger_event, next_json, send_next, subscription_variable,
        };

        use super::*;

        async fn rescan(wallet: &mut Wallet, url: &str) {
            let commit = wallet.shielded_rescan_plan().run(url).await.unwrap();
            wallet.commit_shielded_rescan(commit).unwrap();
        }

        /// The whole point of the rescan: the sync that first met the output
        /// had no way to claim it and collapsed the leaf, and the cursor has
        /// moved past that event. Registering the rebuilt coin and replaying
        /// from the first event claims it without decrypting anything.
        #[tokio::test]
        async fn rescan_claims_a_coin_registered_after_the_sync_passed_its_output() {
            let (listener, url) = bind().await;
            let mut wallet = test_wallet(None);
            let coin = unreachable_coin(9, 42);
            let raw = ciphertextless_output(&wallet.seed, &coin, 0);

            let server = tokio::spawn(async move {
                // The same one-event stream for each of the two rescans. A
                // rescan subscribes twice: the reader of the events before
                // the fork stops at the ledger 9 event, and the ledger 9
                // replay goes on from it.
                for _ in 0..4 {
                    let (mut ws, sub) = accept_subscriber(&listener).await;
                    assert_eq!(
                        subscription_variable(&sub, "id"),
                        0,
                        "a rescan must replay from the first event, whatever the cursor says"
                    );
                    send_next(&mut ws, &sub, ledger_event("zswapLedgerEvents", 1, &raw, 1)).await;
                    while next_json(&mut ws).await.is_some() {}
                }
            });

            rescan(&mut wallet, &url).await;
            assert!(
                wallet.spendable_shielded_coins().is_empty(),
                "an unregistered coin with no ciphertext is not discoverable"
            );
            assert_eq!(wallet.zswap_event_id, 1);

            wallet.watch_for_coins([coin]).unwrap();
            rescan(&mut wallet, &url).await;

            let claimed = wallet.spendable_shielded_coins();
            assert_eq!(claimed.len(), 1, "the registered coin must be claimed");
            assert_eq!(claimed[0].value, 42);
            assert_eq!(claimed[0].nonce, [9u8; 32]);
            assert!(
                wallet.registered_coins().is_empty(),
                "a claimed registration is consumed"
            );

            // Listing the coin is not enough: a spend needs its Merkle path,
            // and the replay collapses the leaf of every output it cannot
            // claim. Building an input is what proves the leaf survived.
            let (_nullifier, qualified) = wallet
                .zswap_state()
                .coins
                .iter()
                .next()
                .expect("claimed coin");
            let (_post_spend, _input) = wallet
                .zswap_state()
                .spend(
                    &mut midnight_helpers::OsRng,
                    &wallet.secret_keys,
                    &qualified,
                    Some(0),
                )
                .expect("a claimed coin must still have a Merkle path to spend from");

            server.await.unwrap();
        }

        /// The ledger consumes a registration when a replay claims it, so the
        /// next rescan has nothing left to claim that coin by. A rescan that
        /// only carried the unclaimed registrations would collapse the coin's
        /// leaf and lose it, which is what registering a second coin (the
        /// "one public nonce per mint" case) would do to the first.
        #[tokio::test]
        async fn a_later_rescan_keeps_the_coins_an_earlier_one_recovered() {
            let (listener, url) = bind().await;
            let mut wallet = test_wallet(None);
            let first = unreachable_coin(9, 42);
            let second = unreachable_coin(8, 77);
            let stream = [
                ciphertextless_output(&wallet.seed, &first, 0),
                ciphertextless_output(&wallet.seed, &second, 1),
            ];

            let server = tokio::spawn(async move {
                // One two-output stream, which each of the two rescans reads
                // twice: once in the reader of the events before the fork,
                // and once in ledger 9.
                for _ in 0..4 {
                    let (mut ws, sub) = accept_subscriber(&listener).await;
                    assert_eq!(subscription_variable(&sub, "id"), 0);
                    for (index, raw) in stream.iter().enumerate() {
                        send_next(
                            &mut ws,
                            &sub,
                            ledger_event("zswapLedgerEvents", index as i64 + 1, raw, 2),
                        )
                        .await;
                    }
                    while next_json(&mut ws).await.is_some() {}
                }
            });

            wallet.watch_for_coins([first]).unwrap();
            rescan(&mut wallet, &url).await;
            assert_eq!(wallet.spendable_shielded_coins().len(), 1);

            // Registering the second coin replays again, which must not cost
            // us the first.
            wallet.watch_for_coins([second]).unwrap();
            rescan(&mut wallet, &url).await;

            let mut values: Vec<u128> = wallet
                .spendable_shielded_coins()
                .iter()
                .map(|c| c.value)
                .collect();
            values.sort_unstable();
            assert_eq!(values, vec![42, 77], "the first coin must survive");

            server.await.unwrap();
        }

        /// The stream of a chain that crossed the fork starts with ledger 8
        /// events. A rescan that started in ledger 9 would fail on the first
        /// one, which is every rescan of such a chain.
        #[tokio::test]
        async fn a_rescan_reads_a_stream_that_starts_with_a_ledger_8_event() {
            let (listener, url) = bind().await;
            let mut wallet = test_wallet(None);
            let coin = unreachable_coin(9, 42);
            let raw = crate::pre_fork::test_support::ciphertextless_output(&wallet.seed, &coin, 0);
            let server = tokio::spawn(async move {
                let (mut ws, sub) = accept_subscriber(&listener).await;
                assert_eq!(subscription_variable(&sub, "id"), 0);
                send_next(&mut ws, &sub, ledger_event("zswapLedgerEvents", 1, &raw, 1)).await;
                while next_json(&mut ws).await.is_some() {}
            });

            wallet.watch_for_coins([coin]).unwrap();
            rescan(&mut wallet, &url).await;

            assert_eq!(wallet.zswap_event_id, 1);
            assert_eq!(wallet.spendable_shielded_coins().len(), 1);
            server.await.unwrap();
        }
    }

    /// A resync's replay starts from the plan's registrations, so one missing
    /// from the state it commits was consumed: the replay claimed the coin,
    /// and here also saw it spent. Putting the registration back would make
    /// every later replay watch for a coin that is gone.
    #[test]
    fn commit_resync_keeps_no_registration_its_replay_consumed() {
        let mut wallet = test_wallet(None);
        wallet.watch_for_coins([unreachable_coin(9, 42)]).unwrap();

        let mut commit = noop_commit(&wallet);
        commit.zswap_state = ZswapLocalState::new();
        wallet.commit_resync(commit).unwrap();

        assert!(wallet.registered_coins().is_empty());
    }

    /// Mock-WebSocket-server test for a replay's resume, driven through the
    /// zswap replay.
    mod resume_ws {
        use midnight_indexer_client::testutil::{
            accept_subscriber, bind, ledger_event, next_json, send_next, subscription_variable,
        };

        use super::*;

        /// A resume asks for the event it already applied, so the indexer
        /// answers at once and its `max_id` says the wallet is at the tip.
        ///
        /// Asking for the next event instead leaves the indexer with nothing
        /// to send, and the replay can only read that silence by waiting out
        /// its 10s timeout. Every build pays that, because
        /// `fund_transaction` resyncs first.
        #[tokio::test]
        async fn zswap_resume_at_the_tip_returns_without_waiting() {
            let (listener, url) = bind().await;
            let server = tokio::spawn(async move {
                let (mut ws, sub) = accept_subscriber(&listener).await;
                let asked = subscription_variable(&sub, "id");
                send_next(&mut ws, &sub, ledger_event("zswapLedgerEvents", 7, "00", 7)).await;
                while next_json(&mut ws).await.is_some() {}
                asked
            });

            let sub_client = SubscriptionClient::new(&url);
            let shielded = ShieldedWallet::<DefaultDB>::default((&test_seed()).into_ledger());
            let started = std::time::Instant::now();
            let replay = replay_zswap_events(
                &sub_client,
                shielded.secret_keys(),
                shielded.state.clone(),
                8,
                true,
                None,
            )
            .await
            .expect("a resume at the tip must succeed");

            assert_eq!(
                replay.last_id, 7,
                "the re-delivered event must not advance the cursor"
            );
            assert!(
                started.elapsed() < std::time::Duration::from_secs(5),
                "took {:?}: the replay waited for the idle timeout instead of reading max_id",
                started.elapsed()
            );
            assert_eq!(
                server.await.unwrap(),
                7,
                "must subscribe at the applied cursor, not past it"
            );
        }
    }
}
