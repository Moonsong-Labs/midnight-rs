use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::helpers;
use helpers::coin_structure::transfer::SenderEvidence;
use helpers::midnight_serialize::tagged_deserialize;
use helpers::mn_ledger::events::EventDetails;
use helpers::mn_ledger::semantics::ZswapLocalStateExt;
use helpers::mn_ledger::structure::{Utxo as LedgerUtxo, UtxoMeta};
use helpers::{
    BlockContext, DefaultDB, DustNullifier, DustWallet, Event, HashOutput, IntoWalletAddress,
    LedgerContext, LedgerParameters, LedgerState, MAX_SUPPLY, Recipient, SecretKeys,
    ShieldedWallet, Sp, Timestamp, UnshieldedTokenType, UnshieldedWallet, Wallet as ContextWallet,
    WalletSeed, WalletState as ZswapLocalState,
};
use midnight_indexer_client::SubscriptionClient;
use midnight_types::{SyncCursors, TrackedUtxo};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use super::pending::{PendingReservations, within_ttl};
use super::types::convert::{IntoLedger, IntoSdk};
use crate::chain_pin::ChainPin;
use crate::replay::{
    DustEventEnvelope, LedgerEventMessage, RECONNECT_MAX_RETRIES, ZswapEventEnvelope,
    already_applied, gave_up, last_applied_before, order_regression, progress_cancelled,
    reconnect_delay, replay_unshielded_events, resume_id, send_progress,
};
use crate::storage::wallet_storage_id;
use crate::{SpentUtxoKey, SyncProgress, WalletError};

/// A Midnight wallet: identity (seed, addresses) and synced ledger state.
///
/// Maintains three streams of state from the indexer:
/// - `zswapLedgerEvents` → shielded coin tracking + Merkle tree
/// - `dustLedgerEvents` → dust/fee UTXO tracking
/// - `unshieldedTransactions` → unshielded UTXO balance
///
/// The state of the SDK's `Wallet` on this generation. Transaction building
/// uses it directly (no full-chain replay).
pub struct Wallet {
    seed: WalletSeed,
    secret_keys: SecretKeys,
    network_id: String,
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
    block_context: Option<BlockContext>,

    /// In-flight reservations: spends built locally but not yet observed
    /// as confirmed on-chain. Applied at [`Wallet::build_context_inner`]
    /// time to prevent local double-builds, cleared when corresponding
    /// events arrive or when the TTL window elapses. Never written to the
    /// confirmed-state files; persisted separately via `pending.json`.
    pending: PendingReservations,

    /// Where this wallet persists its state, when [`Wallet::sync_inner`] was
    /// given a storage directory. Retained so [`Wallet::resync`] can re-save
    /// the moved cursors and [`Wallet::reserve_pending`] can persist
    /// `pending.json` without the caller re-supplying the path.
    storage_dir: Option<PathBuf>,

    /// The indexer this wallet synced against.
    ///
    /// The cursors are event counts, so they mean nothing against a different
    /// server: a replay resumed elsewhere would apply another indexer's events
    /// to this state. Holding the URL here keeps every later replay on the
    /// wallet's own indexer rather than one a consumer supplies.
    ///
    /// It binds nothing across a restart. The snapshot records no indexer
    /// identity, so `Wallet::sync(other_url, ..).with_storage(dir)` resumes
    /// these cursors against `other_url` and nothing notices.
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

/// Construct a `BlockContext` anchored at the given `tblock`.
fn block_context_at(tblock: Timestamp) -> BlockContext {
    BlockContext {
        tblock,
        tblock_err: 30,
        parent_block_hash: Default::default(),
        last_block_time: tblock,
    }
}

/// The window within which a dust-event-anchored `tblock` candidate is still
/// valid relative to the chain's current time: the tighter of the intent
/// `global_ttl` and the dust `dust_grace_period`. The chain enforces both, but
/// the dust grace window (often much shorter than `global_ttl`) is what rejects
/// a stale `ctime` with `OutOfDustValidityWindow`, so it must bound the anchor.
fn anchor_window(
    global_ttl: helpers::Duration,
    dust_grace_period: helpers::Duration,
) -> helpers::Duration {
    if global_ttl.as_seconds() <= dust_grace_period.as_seconds() {
        global_ttl
    } else {
        dust_grace_period
    }
}

/// Hex-decode the event a `LedgerEventMessage` carries.
fn event_bytes(msg: &LedgerEventMessage, kind: &str) -> Result<Vec<u8>, WalletError> {
    hex::decode(&msg.raw).map_err(|e| WalletError::Sync(format!("decode {kind} event hex: {e}")))
}

/// Tagged-deserialize an event of this generation.
fn decode_event(raw: &[u8], kind: &str) -> Result<Event<DefaultDB>, WalletError> {
    tagged_deserialize(raw).map_err(|e| WalletError::Sync(format!("deserialize {kind} event: {e}")))
}

/// Hex-decode and tagged-deserialize the `ledger_parameters` carried on an
/// indexer block. Both initial sync and resync read parameters from the
/// latest block so governance changes (fees, TTL, dust rates) take effect.
fn decode_ledger_parameters(
    block: &midnight_indexer_client::Block,
) -> Result<LedgerParameters, WalletError> {
    let params_hex = block
        .ledger_parameters
        .as_deref()
        .ok_or_else(|| WalletError::Sync("latest block has no ledger_parameters".into()))?;
    let params_bytes = hex::decode(params_hex)
        .map_err(|e| WalletError::Sync(format!("decode ledger params hex: {e}")))?;
    let parameters: LedgerParameters = tagged_deserialize(&params_bytes[..])
        .map_err(|e| WalletError::Sync(format!("deserialize ledger params: {e}")))?;
    validate_ledger_parameters(&parameters)?;
    Ok(parameters)
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
    use helpers::base_crypto::cost_model::FixedPoint;

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

/// This generation's half of [`crate::ResyncPlan`].
#[derive(Clone)]
pub struct ResyncPlan {
    pub(crate) secret_keys: SecretKeys,
    pub(crate) unshielded_address: String,
    pub(crate) dust_wallet: DustWallet<DefaultDB>,
    pub(crate) dust_event_id: i64,
    pub(crate) zswap_state: ZswapLocalState<DefaultDB>,
    pub(crate) zswap_event_id: i64,
    pub(crate) unshielded_utxos: Vec<TrackedUtxo>,
    pub(crate) last_tx_id: Option<i64>,
}

impl ResyncPlan {
    /// The replay phase of [`crate::ResyncPlan::run`] on this generation. It
    /// returns [`WalletError::LedgerMismatch`] when the chain moved to a
    /// later generation, for the caller to cross.
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
        let (unshielded_utxos, last_tx_id, last_block_height, spent_unshielded) = unshielded_res?;
        let block = block_res?;
        // A chain that moved to a later generation since the plan was taken
        // needs the wallet to cross, which is not this generation's to do.
        let chain = crate::LedgerVersion::of_block(&block)?;
        if let Some(found) = [Some(chain), zswap.later_ledger.map(|(_, l)| l)]
            .into_iter()
            .chain([dust.later_ledger.map(|(_, l)| l)])
            .flatten()
            .find(|l| *l != super::LEDGER)
        {
            return Err(WalletError::LedgerMismatch {
                expected: super::LEDGER,
                found,
            });
        }
        let DustReplay {
            wallet: dust_wallet,
            last_id: dust_event_id,
            last_block_time: last_dust_block_time,
            spend_nullifiers: dust_nullifiers,
            ..
        } = dust;
        let ZswapReplay {
            state: zswap_state,
            last_id: zswap_event_id,
            ..
        } = zswap;
        let tblock_ms = block
            .timestamp
            .ok_or_else(|| WalletError::Sync("latest block has no timestamp".into()))?;
        let chain_tblock = Timestamp::from_secs((tblock_ms / 1000) as u64);
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

/// This generation's half of [`crate::ResyncCommit`].
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

/// This generation's half of [`crate::ShieldedRescanPlan`].
pub struct ShieldedRescanPlan {
    pub(crate) secret_keys: SecretKeys,
    pub(crate) initial_state: ZswapLocalState<DefaultDB>,
}

impl ShieldedRescanPlan {
    /// A rescan for `seed`'s wallet that claims `coins` on the way.
    pub(crate) fn new(seed: &WalletSeed, coins: &[crate::CoinInfo]) -> Self {
        let secret_keys = zswap_keys(seed);
        let coin_public_key = secret_keys.coin_public_key();
        let initial_state = coins.iter().fold(empty_zswap_state(seed), |state, coin| {
            state.watch_for(&coin_public_key, &coin.into_ledger())
        });
        Self {
            secret_keys,
            initial_state,
        }
    }

    /// The replay of [`crate::ShieldedRescanPlan::run`] on this generation.
    /// It returns [`WalletError::LedgerMismatch`] when the stream holds
    /// another generation's events.
    pub async fn run(self, indexer_url: &str) -> Result<ShieldedRescanCommit, WalletError> {
        let sub_client = SubscriptionClient::new(indexer_url);
        let replay = replay_zswap_events(
            &sub_client,
            &self.secret_keys,
            self.initial_state,
            0,
            false,
            None,
        )
        .await?;
        if let Some((_, found)) = replay.later_ledger {
            return Err(WalletError::LedgerMismatch {
                expected: super::LEDGER,
                found,
            });
        }
        Ok(ShieldedRescanCommit {
            zswap_state: replay.state,
            zswap_event_id: replay.last_id,
        })
    }
}

/// This generation's half of [`crate::ShieldedRescanCommit`].
#[must_use = "apply with Wallet::commit_shielded_rescan, or the completed replay is discarded"]
pub struct ShieldedRescanCommit {
    pub(crate) zswap_state: ZswapLocalState<DefaultDB>,
    pub(crate) zswap_event_id: i64,
}

/// Who a sync is for, and where it keeps what it learns.
pub(crate) struct SyncIdentity<'a> {
    pub indexer_url: &'a str,
    pub seed: WalletSeed,
    pub address: &'a str,
    pub network_id: &'a str,
    pub storage_dir: Option<&'a Path>,
    /// Whether the sync writes its state under `storage_dir` as it goes:
    /// Dust checkpoints, the pending reservations it loads, and the synced
    /// state. A sync whose state waits for a commit writes nothing, and the
    /// commit saves it.
    pub save: bool,
    /// The finalized block the node reported for this sync, persisted so a
    /// later resume can ask whether it is still on this chain.
    pub chain_pin: Option<ChainPin>,
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
    /// From an empty Dust state, at `next_id`: a sync with no snapshot, or
    /// one whose snapshot predates the hard fork that emptied the chain's
    /// Dust state.
    Fresh { next_id: i64 },
}

/// The shielded keys `seed` derives in this generation.
pub(crate) fn zswap_keys(seed: &WalletSeed) -> SecretKeys {
    ShieldedWallet::<DefaultDB>::default(seed.clone())
        .secret_keys()
        .clone()
}

/// The shielded state of a wallet that has applied no event.
pub(crate) fn empty_zswap_state(seed: &WalletSeed) -> ZswapLocalState<DefaultDB> {
    ShieldedWallet::<DefaultDB>::default(seed.clone()).state
}

impl Wallet {
    /// See [`crate::Wallet::snapshot_dir`].
    pub fn snapshot_dir(&self) -> Option<std::path::PathBuf> {
        let dir = self.storage_dir.as_deref()?;
        Some(crate::storage::snapshot_path(
            dir,
            &self.network_id,
            &wallet_storage_id(&self.unshielded_address),
        ))
    }

    /// See [`crate::Wallet::chain_pin`].
    pub fn chain_pin(&self) -> Option<&ChainPin> {
        self.chain_pin.as_ref()
    }

    /// The indexer this wallet synced from, and the only one its cursors
    /// mean anything against.
    pub fn indexer_url(&self) -> &str {
        &self.indexer_url
    }

    /// See [`crate::Wallet::set_chain_pin`].
    pub fn set_chain_pin(&mut self, pin: ChainPin) {
        self.chain_pin = Some(pin);
    }

    /// Finish a sync on this generation: replay the Dust stream from the
    /// cursor `start` carries, and assemble the wallet.
    ///
    /// The neutral sync has replayed the shielded and unshielded streams
    /// already, crossing each hard fork it met, and hands over the shielded
    /// state in this generation. `block` is the chain's latest block, whose
    /// ledger parameters are in this generation.
    pub(crate) async fn finish_sync(
        sync: SyncIdentity<'_>,
        block: &midnight_indexer_client::Block,
        start: SyncStart,
        progress: Option<mpsc::Sender<SyncProgress>>,
    ) -> Result<Self, WalletError> {
        let SyncIdentity {
            indexer_url,
            seed,
            address,
            network_id,
            storage_dir,
            save,
            chain_pin,
        } = sync;
        let save_dir = storage_dir.filter(|_| save);
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
        let secret_keys = zswap_keys(&seed);

        let parameters = decode_ledger_parameters(block)?;
        let block_timestamp = block
            .timestamp
            .map(|ms| Timestamp::from_secs((ms / 1000) as u64))
            .ok_or_else(|| WalletError::Sync("latest block has no timestamp".into()))?;

        let (dust_wallet, start_dust_id) = match dust {
            DustStart::Resume { wallet, next_id } => (*wallet, next_id),
            DustStart::Fresh { next_id } => (
                DustWallet::default(seed.clone(), Some(&parameters)),
                next_id,
            ),
        };

        let sub_client = SubscriptionClient::new(indexer_url);
        let dust_checkpoint = make_dust_checkpoint(
            save_dir,
            network_id,
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
        if let Some((_, found)) = dust.later_ledger {
            return Err(WalletError::LedgerMismatch {
                expected: super::LEDGER,
                found,
            });
        }
        let DustReplay {
            wallet: dust_wallet,
            last_id: dust_event_id,
            last_block_time: last_dust_block_time,
            spend_nullifiers: dust_nullifiers,
            ..
        } = dust;

        // See `resync` for the full discussion of the anchor selection. Prefer
        // `last_dust_block_time + 1s` (race-safe) while it is still inside the
        // dust validity window relative to the chain's current time, falling
        // back to `block_timestamp` for devnet's hardcoded-genesis case.
        let window = anchor_window(parameters.global_ttl, parameters.dust.dust_grace_period);
        let candidate = last_dust_block_time.map(|t| t + helpers::Duration::from_secs(1));
        let block_tblock = match candidate {
            Some(t) if t + window >= block_timestamp => t,
            _ => block_timestamp,
        };
        let block_context = Some(block_context_at(block_tblock));

        info!(
            zswap_event_id,
            dust_event_id,
            unshielded_utxos = unshielded_utxos.len(),
            height = last_block_height,
            ledger = %super::LEDGER,
            "wallet synced"
        );

        // Load any pre-existing pending reservations from disk so they
        // survive process restarts. Confirmed-state files never carry
        // pending entries; this is a separate file.
        let pending = match save_dir {
            Some(dir) => {
                PendingReservations::load(dir, network_id, &wallet_id)?.unwrap_or_default()
            }
            None => PendingReservations::default(),
        };

        let mut state = Self {
            seed,
            secret_keys,
            network_id: network_id.to_string(),
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
            pending,
            storage_dir: storage_dir.map(Path::to_path_buf),
            indexer_url: indexer_url.to_string(),
        };

        // Reservations made before a restart whose spends this replay just
        // observed confirmed are no longer in flight; drop them so the
        // underlying UTXOs become spendable again immediately.
        state
            .pending
            .clear_confirmed(&spent_unshielded, &dust_nullifiers);

        // Any pending entry whose TTL window has elapsed against the chain's
        // current view can no longer produce a valid transaction; drop them
        // so they don't pollute subsequent build contexts.
        if let Some(ref bc) = state.block_context {
            state
                .pending
                .evict_expired(bc.tblock, state.parameters.global_ttl);
        }

        if let Some(dir) = save_dir {
            state.save(dir)?;
        }

        Ok(state)
    }

    /// Whether the dust state has been synced (required for transaction building).
    pub fn dust_synced(&self) -> bool {
        self.dust_event_id > 0
    }

    /// Record the dust + unshielded spends of a freshly-built (and typically
    /// about-to-be-submitted) transaction so subsequent in-process builds
    /// don't re-select the same inputs.
    ///
    /// Dust and unshielded reservations live in `Wallet::pending` until either:
    /// - event replay (a sync, or [`Wallet::resync`]) observes
    ///   the corresponding confirmed spends and clears them,
    /// - or their TTL window elapses (evicted at [`Wallet::build_context_inner`]
    ///   time).
    ///
    /// `shielded_spends` (Zswap coin nullifiers pinned by a contract call) are
    /// cleared by TTL only: once the spend confirms, resync drops the coin from
    /// `zswap_state`, so filtering it out becomes a no-op regardless.
    ///
    /// `reserved_at` should be the chain time (typically the same anchor used
    /// to build the transaction); TTL eviction compares against the chain's
    /// `block_context.tblock`. Confirmed-state files never persist these
    /// reservations, they live in `pending.json` only and are dropped from
    /// disk once `Wallet::pending` becomes empty.
    ///
    /// When the wallet was synced with a storage directory, the updated
    /// pending set is persisted to `pending.json` immediately so a crash
    /// between build and confirmation does not lose the reservation. The
    /// write is best-effort: a failure is logged and the in-memory
    /// reservation stands, since the transaction was already built.
    pub fn reserve_pending(
        &mut self,
        dust_batches: Vec<super::types::DustSpendBatch>,
        unshielded_spends: Vec<SpentUtxoKey>,
        shielded_spends: Vec<helpers::Nullifier>,
        reserved_at: Timestamp,
    ) {
        self.pending.reserve(
            dust_batches,
            unshielded_spends,
            shielded_spends,
            reserved_at,
        );

        // Persist only the pending file: a full `save` would rewrite the
        // multi-MB confirmed-state files on every transfer. The write is
        // best-effort because erroring here would strand a transaction that
        // was already built; the in-memory reservation still protects the
        // running process, and the same disk fault will fail loudly at the
        // next resync's hard `save`. Crash-safety is degraded until then,
        // hence the error-level log.
        if let Some(dir) = self.storage_dir.as_deref()
            && let Err(err) = self.pending.save(dir, &self.network_id, &self.storage_id())
        {
            error!(error = %err, "failed to persist pending reservations; reservation held in memory only");
        }
    }

    /// Hand back the inputs a build reserved. See [`crate::Wallet::release`].
    ///
    /// Dust is named by nullifier rather than by batch so that a path which
    /// never produced a [`TransferResult`](crate::TransferResult), such as
    /// sponsoring or a deploy, can still hand its reservation back.
    pub fn release_pending(
        &mut self,
        dust_nullifiers: &[helpers::DustNullifier],
        unshielded_spends: &[SpentUtxoKey],
        shielded_spends: &[helpers::Nullifier],
        reserved_at: Timestamp,
    ) {
        self.pending.release(
            dust_nullifiers,
            unshielded_spends,
            shielded_spends,
            reserved_at,
        );

        if let Some(dir) = self.storage_dir.as_deref()
            && let Err(err) = self.pending.save(dir, &self.network_id, &self.storage_id())
        {
            error!(error = %err, "failed to persist released reservations; release held in memory only");
        }
    }

    /// [`Self::release_pending`] for what a build reported spending.
    pub fn release(&mut self, spent: &crate::SpentInputs) {
        let dust: Vec<DustNullifier> = spent.dust.iter().map(|n| n.into_ledger()).collect();
        let shielded: Vec<helpers::Nullifier> =
            spent.shielded.iter().map(|n| n.into_ledger()).collect();
        self.release_pending(&dust, &spent.unshielded, &shielded, spent.reserved_at);
    }

    /// See [`crate::Wallet::has_observed`].
    pub fn has_observed(&self, spent: &[crate::SpentInputs]) -> bool {
        spent.iter().all(|spent| {
            let unshielded = spent.unshielded.iter().all(|key| {
                !self.unshielded_utxos.iter().any(|utxo| {
                    utxo.intent_hash.as_deref() == Some(key.intent_hash.as_str())
                        && utxo.output_index == Some(i64::from(key.output_index))
                })
            });
            let shielded = spent.shielded.iter().all(|nullifier| {
                !self
                    .zswap_state
                    .coins
                    .contains_key(&nullifier.into_ledger())
            });
            // The lookup hides a UTXO whose `pending_until` is set. Only a
            // spend on a clone sets it, and this state holds replayed events.
            let dust = self
                .dust_wallet
                .dust_local_state
                .as_ref()
                .is_none_or(|state| {
                    spent.dust.iter().all(|nullifier| {
                        state
                            .find_utxo_by_nullifier(nullifier.into_ledger())
                            .is_none()
                    })
                });
            unshielded && shielded && dust
        })
    }

    /// This wallet's on-disk identity; see [`wallet_storage_id`].
    fn storage_id(&self) -> String {
        wallet_storage_id(&self.unshielded_address)
    }

    /// Nullifiers of shielded coins reserved by recent, still-pending builds,
    /// so [`Wallet::spendable_shielded_coins`] can exclude them (the build
    /// context excludes them from Zswap coin selection directly).
    pub(crate) fn reserved_shielded_nullifiers(&self) -> impl Iterator<Item = &helpers::Nullifier> {
        self.pending.shielded_nullifiers()
    }

    /// Nullifiers of the Dust UTXOs that recent, still-pending builds reserved.
    ///
    /// A reservation outside [`within_ttl`] at `block_context.tblock` does not
    /// count, because [`Self::add_funding`] evicts it before a build selects.
    /// With no block context, every reservation counts, as no eviction runs.
    pub(crate) fn reserved_dust_nullifiers(&self) -> impl Iterator<Item = &DustNullifier> {
        let now = self.block_context.as_ref().map(|bc| bc.tblock);
        let global_ttl = self.parameters.global_ttl;
        self.pending
            .dust_batches()
            .filter(move |batch| {
                now.is_none_or(|now| within_ttl(batch.reserved_at, now, global_ttl))
            })
            .flat_map(|batch| batch.spends.iter().map(|spend| &spend.old_nullifier))
    }

    /// See [`crate::Wallet::save`].
    pub fn save(&self, base: &Path) -> Result<(), WalletError> {
        let wallet_id = self.storage_id();
        super::snapshot::save(
            base,
            &self.network_id,
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
        self.pending.save(base, &self.network_id, &wallet_id)
    }

    /// The ledger state a build executes against: the chain's parameters and
    /// genesis settings, the proving resolver, and the latest block context.
    ///
    /// Holds no key material and no coin state, so nothing here depends on
    /// which wallet pays. Add a funding view with [`Self::add_funding`].
    ///
    /// Performs no I/O. The caller keeps the wallet synced (typically via
    /// `MidnightProvider::resync_wallet`) and refreshes [`Self::block_context`]
    /// first, since the embedded `block_context.tblock` drives proof root
    /// lookup and transaction TTL.
    pub fn execution_context(&self) -> Result<Arc<LedgerContext<DefaultDB>>, WalletError> {
        // reserve_pool must equal MAX_SUPPLY to satisfy the NIGHT balance invariant.
        let ledger_state = LedgerState::with_genesis_settings(
            &self.network_id,
            self.parameters.clone(),
            0,
            MAX_SUPPLY,
            0,
        )
        .map_err(|e| WalletError::Sync(format!("construct ledger state: {e:?}")))?;

        Ok(Arc::new(LedgerContext {
            ledger_state: std::sync::Mutex::new(Sp::new(ledger_state)),
            wallets: std::sync::Mutex::new(std::collections::HashMap::new()),
            resolver: tokio::sync::Mutex::new(&*helpers::context::DEFAULT_RESOLVER),
            latest_block_context: std::sync::Mutex::new(self.block_context.clone()),
        }))
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
    /// wallet. It also re-anchors `ctx`'s block context on the chain view the
    /// funding snapshot came from, so a resync between the two halves cannot
    /// leave the dust state newer than the `ctime` a build reads.
    ///
    /// The only mutation is TTL eviction of expired `pending` entries against
    /// `block_context.tblock`: entries whose `reserved_at + global_ttl` window
    /// has elapsed cannot produce a valid transaction, and would block the
    /// underlying UTXOs forever otherwise.
    pub fn add_funding(&mut self, ctx: &LedgerContext<DefaultDB>) -> Result<(), WalletError> {
        // One funding view per context. Funding twice would merge a fresh UTXO
        // set into the one already there: the second pass skips what a pending
        // build reserved, but cannot withdraw what the first pass inserted, so
        // a spent input stays selectable.
        if ctx
            .wallets
            .lock()
            .map_err(|_| WalletError::Sync("wallets lock poisoned".into()))?
            .contains_key(&self.seed)
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
            self.pending
                .evict_expired(bc.tblock, self.parameters.global_ttl);
        }

        // Re-anchor the context on the chain view this funding snapshot came
        // from. The execution half captured `block_context` when it was built,
        // and a resync since then leaves the dust state below newer than that
        // anchor. A build reads `ctime` from the anchor and witnesses against
        // the newer Dust root, which the chain rejects because it looks the
        // root up as `root_history.get(ctime)`.
        *ctx.latest_block_context
            .lock()
            .map_err(|_| WalletError::Sync("block context lock poisoned".into()))? =
            self.block_context.clone();

        // Populate UTXO state so the transaction builder can find our UTXOs.
        let unshielded = UnshieldedWallet::default(self.seed.clone());
        let owner = unshielded.user_address;
        let utxo_ctime = self
            .block_context
            .as_ref()
            .map(|bc| Timestamp::from_secs(bc.tblock.to_secs().saturating_sub(3600)))
            .unwrap_or_else(|| Timestamp::from_secs(0));

        // Filter out UTXOs reserved by recent (still-pending) builds so the
        // selector doesn't re-pick them before the indexer confirms the
        // spend.
        let pending_unshielded: std::collections::HashSet<(String, i64)> = self
            .pending
            .unshielded_keys()
            .map(|k| (k.intent_hash.clone(), k.output_index as i64))
            .collect();

        {
            let mut guard = ctx
                .ledger_state
                .lock()
                .map_err(|_| WalletError::Sync("ledger state lock poisoned".into()))?;
            let mut ledger_state = (**guard).clone();

            // intent_hash + output_no are part of a UTXO's identity; falling back
            // to default values silently creates collisions between distinct UTXOs
            // and synthesizes inputs the chain will reject.
            let mut utxo_state = (*ledger_state.utxo).clone();
            for tracked in &self.unshielded_utxos {
                let key = match (&tracked.intent_hash, tracked.output_index) {
                    (Some(h), Some(idx)) => Some((h.clone(), idx)),
                    _ => None,
                };
                if let Some(k) = key
                    && pending_unshielded.contains(&k)
                {
                    continue;
                }
                let utxo = tracked_to_ledger_utxo(tracked, owner)?;
                utxo_state = utxo_state.insert(utxo, UtxoMeta { ctime: utxo_ctime });
            }
            ledger_state.utxo = Sp::new(utxo_state);
            *guard = Sp::new(ledger_state);
        }

        // Insert wallet with our synced state. Pending dust reservations are
        // re-applied via `mark_spent` so the fee selector skips them; they
        // live only on this LedgerContext clone — `self.dust_wallet` itself
        // retains only events confirmed by the indexer. Each pending entry
        // carries its post-spend `DustLocalState`; applying them in
        // chronological order leaves the clone's `dust_local_state` at the
        // most recent post-pending value.
        {
            let mut shielded = ShieldedWallet::<DefaultDB>::default(self.seed.clone());
            shielded.state = self.zswap_state.clone();

            // Drop coins a recent still-pending build already spent so the
            // Zswap selector (`min_match_coin`) can't re-pick them before the
            // indexer confirms the spend. `remove` on an already-absent
            // nullifier (the spend confirmed and resync dropped the coin) is a
            // harmless no-op.
            for nullifier in self.pending.shielded_nullifiers() {
                shielded.state.coins = shielded.state.coins.remove(nullifier);
            }

            // Add pending-spend nullifiers to the dust wallet's `spent_utxos`
            // set so speculative_spend skips them — but DO NOT overwrite
            // `dust_local_state` with the prior tx's post-spend tree.
            //
            // The new `DustWallet::mark_spent(spends, updated_state)` API in
            // ledger-helpers 8.1.0-rc.1 took the previous single-arg form
            // `mark_spent(spends)` and bolted on a state overwrite. If we apply
            // the speculative `updated_state`, the dust commitment tree the
            // proof witnesses against is the wallet's projected post-spend
            // tree, which has no corresponding entry in the chain's
            // `root_history` until the prior tx has been processed at the
            // chain-block level — and even then it only matches if no other
            // dust events landed in the same block. Re-passing the current
            // state makes the overwrite a no-op while still adding the
            // nullifiers, keeping the witnessed root aligned with the chain's
            // `root_history.get(ctime)` lookup at the proof's declared
            // timestamp.
            let mut dust = self.dust_wallet.clone();
            match dust.dust_local_state.clone() {
                Some(state) => {
                    for batch in self.pending.dust_batches() {
                        dust.mark_spent(&batch.spends, state.clone());
                    }
                }
                // No construction path in this crate produces `None` here
                // alongside pending batches: every `DustWallet` is built
                // with `Some(&parameters)`, so even a pre-registration
                // wallet carries `Some(empty)` state. `None` is only
                // reachable via deserialized/legacy or manually-mutated
                // state, and with nothing pending there is nothing to
                // replay. The guard below is defensive: pending dust
                // reservations with no state to apply them to would
                // silently disable double-build prevention, so refuse and
                // let the caller sync first.
                None => {
                    let pending_dust = self.pending.dust_batches().count();
                    if pending_dust > 0 {
                        return Err(WalletError::Transfer(format!(
                            "wallet has {pending_dust} pending dust reservation(s) but no dust \
                             state; wait for dust sync before building"
                        )));
                    }
                }
            }

            let wallet = ContextWallet {
                root_seed: Some(self.seed.clone()),
                shielded,
                unshielded: helpers::UnshieldedWallet::default(self.seed.clone()),
                dust,
            };

            ctx.wallets
                .lock()
                .map_err(|_| WalletError::Sync("wallets lock poisoned".into()))?
                .insert(self.seed.clone(), wallet);
        }

        Ok(())
    }

    /// [`Self::execution_context`] carrying this wallet's own funding view,
    /// for the builds that fund from the wallet that builds them.
    pub fn build_context_inner(&mut self) -> Result<Arc<LedgerContext<DefaultDB>>, WalletError> {
        let ctx = self.execution_context()?;
        self.add_funding(&ctx)?;
        Ok(ctx)
    }

    // -------------------------------------------------------------------------
    // Accessors
    // -------------------------------------------------------------------------

    /// See [`crate::Wallet::last_block_height`].
    pub fn last_block_height(&self) -> i64 {
        self.last_block_height
    }

    pub fn last_tx_id(&self) -> Option<i64> {
        self.last_tx_id
    }

    pub fn zswap_event_id(&self) -> i64 {
        self.zswap_event_id
    }

    pub fn dust_event_id(&self) -> i64 {
        self.dust_event_id
    }

    /// See [`crate::Wallet::sync_cursors`].
    pub fn sync_cursors(&self) -> SyncCursors {
        SyncCursors {
            last_block_height: self.last_block_height,
            last_tx_id: self.last_tx_id,
            zswap_event_id: self.zswap_event_id,
            dust_event_id: self.dust_event_id,
        }
    }

    pub fn seed(&self) -> &WalletSeed {
        &self.seed
    }

    /// See [`crate::Wallet::shielded_public_keys`].
    pub fn shielded_public_keys(&self) -> (crate::CoinPublicKey, crate::EncryptionPublicKey) {
        (
            self.secret_keys.coin_public_key().into_sdk(),
            self.secret_keys.enc_public_key().into_sdk(),
        )
    }

    /// See [`crate::Wallet::network`].
    pub fn network(&self) -> &str {
        &self.network_id
    }

    /// See [`crate::Wallet::unshielded_address`].
    pub fn unshielded_address(&self) -> String {
        self.unshielded_address.clone()
    }

    /// See [`crate::Wallet::shielded_address`].
    pub fn shielded_address(&self) -> String {
        ShieldedWallet::<DefaultDB>::default(self.seed.clone())
            .address(&self.network_id)
            .to_bech32()
    }

    pub fn unshielded_utxos(&self) -> &[TrackedUtxo] {
        &self.unshielded_utxos
    }

    pub fn parameters(&self) -> &LedgerParameters {
        &self.parameters
    }

    /// The parts of [`Self::parameters`] every generation shares.
    pub fn chain_parameters(&self) -> crate::ChainParameters {
        (&self.parameters).into_sdk()
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
        }
    }

    /// See [`crate::Wallet::commit_resync`].
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
        // overwrite them. Resync runs before every transfer/contract build
        // (`MidnightProvider::resync_wallet`) and on user polling, so the
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
        let pending_before =
            self.pending.dust_batches().count() + self.pending.unshielded_keys().count();
        // Captured before the overwrite for the same reason the pending set
        // is merged rather than replaced: a registration added after this
        // resync's plan was snapshotted is not in the replayed state.
        let registrations = self.watched_coins();

        self.dust_wallet = dust_wallet;
        self.dust_event_id = dust_event_id;
        self.zswap_state = zswap_state;
        self.zswap_event_id = zswap_event_id;
        self.carry_registrations(registrations);
        self.unshielded_utxos = unshielded_utxos;
        self.last_tx_id = Some(last_tx_id);
        // Only advance last_block_height if the unshielded sync actually saw a
        // newer block. Without this guard, a resume with no new unshielded txs
        // would clobber the persisted height with 0 (the default returned by
        // `replay_unshielded_events` when no events arrive).
        if last_block_height > self.last_block_height {
            self.last_block_height = last_block_height;
        }
        // Refresh parameters from the latest block so governance changes to
        // fees/TTL/dust rates take effect. Assigned before `global_ttl` is
        // read below so the anchor math uses the fresh value.
        self.parameters = parameters;

        // `block_context.tblock` drives both the proof's `DustActions.ctime`
        // and the intent's `ttl = tblock + global_ttl`. The chain checks:
        //
        //   1. `root_history.get(ctime)` matches our DustLocalState root, and
        //   2. `ttl >= chain.current_tblock` at apply time.
        //
        // Constraint (1) wants the most recent block_time we know matches the
        // chain's root: `last_dust_block_time + 1s` (root_history only changes
        // on dust events, so any time in the gap returns the entry at our
        // last seen event).
        //
        // Constraint (2) wants `tblock` close to the chain's current time. On
        // devnet, where genesis is hardcoded months before wall clock but the
        // chain runs in real time, `last_dust_block_time` from a genesis event
        // is too old: `last_dust + global_ttl` is already in the past.
        //
        // Prefer the most recent race-safe candidate that still has a valid
        // TTL window: `last_dust_block_time + 1s` if we observed new events,
        // else the previous block_context anchor (still race-safe because our
        // state hasn't changed since then). Fall back to `chain_tblock` only
        // when neither has a TTL window that covers the chain's current time;
        // that fallback accepts a small race window (a dust event indexed
        // between our replay's tip and `get_block`) but is required when chain
        // time has advanced past `candidate + global_ttl` — e.g. on devnet
        // where genesis is hardcoded months before wall clock.
        // The candidate is only safe to use if it still falls inside the
        // chain's *dust* validity window: the node checks
        // `ctime + dust_grace_period >= tblock` against the current block time.
        // `global_ttl` (the intent TTL, often days) is the wrong bound here —
        // it can be far larger than `dust_grace_period` (e.g. 14d vs 3h on
        // devnet), which would keep a stale candidate that the node's 3h dust
        // window then rejects with `OutOfDustValidityWindow`. Clamp to the
        // tighter of the two so a candidate older than the dust grace period
        // falls back to `chain_tblock`.
        let window = anchor_window(
            self.parameters.global_ttl,
            self.parameters.dust.dust_grace_period,
        );
        let candidate = last_dust_block_time
            .map(|t| t + helpers::Duration::from_secs(1))
            .or_else(|| self.block_context.as_ref().map(|bc| bc.tblock));
        let tblock = match candidate {
            Some(t) if t + window >= chain_tblock => t,
            _ => chain_tblock,
        };
        self.block_context = Some(block_context_at(tblock));

        // Reservations whose spends this replay just observed confirmed are
        // no longer in flight; drop them so the underlying UTXOs become
        // spendable again immediately instead of waiting for TTL eviction.
        self.pending
            .clear_confirmed(&spent_unshielded, &dust_nullifiers);
        let pending_changed = self.pending.dust_batches().count()
            + self.pending.unshielded_keys().count()
            != pending_before;

        // Re-persist the committed state (moved cursors, refreshed
        // parameters, cleared pending set) so a crash before the next sync
        // resumes from here. Must run after `clear_confirmed` above: `save`
        // rewrites (or removes) `pending.json` from the in-memory set.
        // Skipped entirely on no-op resyncs (see the dirty-check above):
        // pre-build resyncs are frequent and must not rewrite the
        // generation files when nothing moved.
        if (cursors_advanced || parameters_changed || pending_changed)
            && let Some(dir) = self.storage_dir.as_deref()
        {
            self.save(dir)?;
        }

        Ok(())
    }

    // -------------------------------------------------------------------------
    // Coins the wallet owns but cannot discover
    // -------------------------------------------------------------------------

    /// See [`crate::Wallet::watch_for_coin`].
    pub fn watch_for_coin(&mut self, coin: crate::CoinInfo) -> Result<(), WalletError> {
        self.watch_for_coins([coin])
    }

    /// [`Self::watch_for_coin`] for several coins, persisting once.
    ///
    /// Registering nothing writes nothing.
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
        match self.storage_dir.as_deref() {
            Some(dir) if registered => self.save(dir),
            _ => Ok(()),
        }
    }

    /// See [`crate::Wallet::forget_coin`].
    pub fn forget_coin(&mut self, coin: crate::CoinInfo) -> Result<(), WalletError> {
        self.forget_coins([coin])
    }

    /// [`Self::forget_coin`] for several coins, persisting once.
    pub fn forget_coins(
        &mut self,
        coins: impl IntoIterator<Item = crate::CoinInfo>,
    ) -> Result<(), WalletError> {
        let recipient = Recipient::User(self.secret_keys.coin_public_key());
        let mut forgot = false;
        for coin in coins {
            let commitment = coin.into_ledger().commitment(&recipient);
            if self.zswap_state.pending_outputs.contains_key(&commitment) {
                self.zswap_state.pending_outputs =
                    self.zswap_state.pending_outputs.remove(&commitment);
                forgot = true;
            }
        }
        match self.storage_dir.as_deref() {
            Some(dir) if forgot => self.save(dir),
            _ => Ok(()),
        }
    }

    /// See [`crate::Wallet::watched_coins`].
    pub fn watched_coins(&self) -> Vec<crate::CoinInfo> {
        // The ledger's map iterates in a deterministic but unspecified order,
        // which callers must not depend on. Sorting gives them one they can.
        let mut coins: Vec<crate::CoinInfo> = self
            .registered_coins()
            .into_iter()
            .map(IntoSdk::into_sdk)
            .collect();
        coins.sort_unstable();
        coins
    }

    fn registered_coins(&self) -> Vec<helpers::CoinInfo> {
        self.zswap_state
            .pending_outputs
            .iter()
            .map(|(_commitment, coin)| *coin)
            .collect()
    }

    /// Whether the wallet's claimed coin set holds `coin`, which it keys by
    /// the nullifier this wallet's secret key derives for it.
    fn holds_coin(&self, coin: &helpers::CoinInfo) -> bool {
        let nullifier = coin.nullifier(&SenderEvidence::User(std::borrow::Cow::Borrowed(
            &self.secret_keys.coin_secret_key,
        )));
        self.zswap_state.coins.contains_key(&nullifier)
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
    fn coins_to_register(&self) -> Vec<helpers::CoinInfo> {
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

    /// Put registrations back after a replayed `zswap_state` replaced the one
    /// that held them, skipping any coin the new state already holds.
    ///
    /// Every path that replaces `zswap_state` takes its input from a replay
    /// that started before the caller could register anything, so without
    /// this a registration made during the replay would be dropped with no
    /// error.
    pub(crate) fn carry_registrations(&mut self, registrations: Vec<crate::CoinInfo>) {
        let coin_public_key = self.secret_keys.coin_public_key();
        for coin in registrations.into_iter().map(IntoLedger::into_ledger) {
            if !self.holds_coin(&coin) {
                self.zswap_state = self.zswap_state.watch_for(&coin_public_key, &coin);
            }
        }
    }

    /// Snapshot the inputs of a shielded rescan's replay phase. See
    /// [`ShieldedRescanPlan`] for the intended plan → run → commit flow.
    pub fn shielded_rescan_plan(&self) -> ShieldedRescanPlan {
        ShieldedRescanPlan::new(&self.seed, &self.rescan_coins())
    }

    /// Every coin a rescan must register before its replay; see
    /// `coins_to_register`.
    pub(crate) fn rescan_coins(&self) -> Vec<crate::CoinInfo> {
        self.coins_to_register()
            .into_iter()
            .map(IntoSdk::into_sdk)
            .collect()
    }

    /// Where this wallet persists its state, when it persists one.
    pub(crate) fn storage_dir(&self) -> Option<&Path> {
        self.storage_dir.as_deref()
    }

    /// See [`crate::Wallet::commit_shielded_rescan`].
    pub fn commit_shielded_rescan(
        &mut self,
        commit: ShieldedRescanCommit,
    ) -> Result<(), WalletError> {
        let ShieldedRescanCommit {
            zswap_state,
            zswap_event_id,
        } = commit;
        let registrations = self.watched_coins();
        self.zswap_state = zswap_state;
        self.zswap_event_id = zswap_event_id;
        self.carry_registrations(registrations);
        if let Some(dir) = self.storage_dir.as_deref() {
            self.save(dir)?;
        }
        Ok(())
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
    /// Where the stream moved to a later ledger generation: the id of that
    /// generation's first event, which this replay did not apply.
    pub later_ledger: Option<(i64, crate::LedgerVersion)>,
}

pub(crate) async fn replay_zswap_events(
    sub_client: &SubscriptionClient,
    secret_keys: &SecretKeys,
    initial_state: ZswapLocalState<DefaultDB>,
    start_id: i64,
    resuming: bool,
    progress: Option<mpsc::Sender<SyncProgress>>,
) -> Result<ZswapReplay, WalletError> {
    use midnight_indexer_client::subscription::queries::ZSWAP_LEDGER_EVENTS_SUBSCRIPTION;

    let mut state = initial_state;
    let mut last_id: i64 = last_applied_before(start_id);
    let mut count: u64 = 0;
    let mut retries: u32 = 0;
    let mut later_ledger = None;
    // Semantic timeout, layered above the client's transport keepalive: the
    // client guarantees a dead socket errors out within its idle timeout
    // (20s), so reaching this bound means the server is alive but sending no
    // events — "already at tip" when resuming, a fatal stall otherwise.
    let event_timeout = if resuming {
        std::time::Duration::from_secs(10)
    } else {
        std::time::Duration::from_secs(30)
    };

    'reconnect: loop {
        let resume_id = resume_id(count, last_id);
        let variables = serde_json::json!({ "id": resume_id });
        let mut subscription = match sub_client
            .subscribe::<ZswapEventEnvelope>(ZSWAP_LEDGER_EVENTS_SUBSCRIPTION, variables)
            .await
        {
            Ok(s) => s,
            Err(e) if e.is_retryable() && retries < RECONNECT_MAX_RETRIES => {
                retries += 1;
                warn!(retries, error = %e, "zswap subscribe failed, retrying");
                tokio::time::sleep(reconnect_delay(retries)).await;
                continue 'reconnect;
            }
            Err(e) => return Err(gave_up("zswap", e)),
        };
        // Highest id delivered on *this* connection; see `order_regression`.
        let mut conn_high: Option<i64> = None;

        loop {
            let event = tokio::time::timeout(event_timeout, subscription.next()).await;

            match event {
                Ok(Some(Ok(envelope))) => {
                    let msg = &envelope.zswap_ledger_events;

                    if let Some(prev) = order_regression(msg.id, conn_high) {
                        return Err(WalletError::EventOrder {
                            kind: "zswap",
                            id: msg.id,
                            prev,
                        });
                    }
                    conn_high = Some(msg.id);

                    if msg.max_id == 0 {
                        debug!("no zswap events on this chain");
                        break 'reconnect;
                    }

                    if already_applied(msg.id, last_id, start_id, count > 0) {
                        debug!(id = msg.id, last_id, "skipping re-delivered zswap event");
                        if msg.id >= msg.max_id {
                            info!(count, last_id, "zswap replay complete");
                            send_progress(&progress, SyncProgress::ZswapComplete { events: count });
                            break 'reconnect;
                        }
                        continue;
                    }

                    let raw = event_bytes(msg, "zswap")?;
                    let ledger = crate::LedgerVersion::of_event(&raw)?;
                    if ledger > super::LEDGER {
                        info!(id = msg.id, %ledger, "zswap events move to a later ledger");
                        later_ledger = Some((msg.id, ledger));
                        break 'reconnect;
                    }
                    if ledger < super::LEDGER {
                        return Err(WalletError::LedgerMismatch {
                            expected: super::LEDGER,
                            found: ledger,
                        });
                    }
                    let ev = decode_event(&raw, "zswap")?;
                    state = state.replay_events(secret_keys, [&ev]).map_err(|e| {
                        WalletError::Sync(format!("replay zswap event id={}: {e}", msg.id))
                    })?;

                    // Only an applied event counts as progress for the
                    // reconnect bound; deduped re-deliveries must not reset it.
                    retries = 0;
                    last_id = msg.id;
                    count += 1;

                    if count.is_multiple_of(10_000) {
                        info!(
                            count,
                            id = msg.id,
                            max_id = msg.max_id,
                            "zswap replay progress"
                        );
                        let alive = send_progress(
                            &progress,
                            SyncProgress::ZswapEvents {
                                current: msg.id,
                                max: msg.max_id,
                            },
                        );
                        if !alive {
                            return Err(progress_cancelled("zswap"));
                        }
                    }

                    if msg.id >= msg.max_id {
                        info!(count, last_id, "zswap replay complete");
                        send_progress(&progress, SyncProgress::ZswapComplete { events: count });
                        break 'reconnect;
                    }
                }
                Ok(Some(Err(e))) if e.is_retryable() && retries < RECONNECT_MAX_RETRIES => {
                    retries += 1;
                    warn!(retries, error = %e, "zswap subscription dropped, reconnecting");
                    tokio::time::sleep(reconnect_delay(retries)).await;
                    continue 'reconnect;
                }
                Ok(Some(Err(e))) => return Err(gave_up("zswap", e)),
                Ok(None) => {
                    if resuming && count == 0 {
                        info!(last_id, "zswap already at tip");
                        send_progress(&progress, SyncProgress::ZswapComplete { events: 0 });
                        break 'reconnect;
                    }
                    // Mid-replay stream end without a `complete`: treat as a
                    // dropped connection and resume from the cursor.
                    if retries < RECONNECT_MAX_RETRIES {
                        retries += 1;
                        warn!(retries, "zswap subscription ended early, reconnecting");
                        tokio::time::sleep(reconnect_delay(retries)).await;
                        continue 'reconnect;
                    }
                    return Err(WalletError::Sync(format!(
                        "zswap subscription ended before replay completed \
                         (after {RECONNECT_MAX_RETRIES} reconnect attempts)"
                    )));
                }
                Err(_) => {
                    if resuming && count == 0 {
                        info!(last_id, "zswap already at tip");
                        send_progress(&progress, SyncProgress::ZswapComplete { events: 0 });
                        break 'reconnect;
                    }
                    return Err(WalletError::Sync("timeout waiting for zswap events".into()));
                }
            }
        }
    }

    Ok(ZswapReplay {
        state,
        last_id,
        later_ledger,
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
    /// Where the stream moved to a later ledger generation; see
    /// [`ZswapReplay::later_ledger`].
    pub later_ledger: Option<(i64, crate::LedgerVersion)>,
}

/// Replay the Dust event stream from `start_id` into `dust_wallet`.
///
/// The replay consumes the events of an earlier generation without applying
/// them. The hard fork to this generation replaced the chain's Dust state
/// with an empty one, so these events describe Dust that no longer exists.
pub(crate) async fn replay_dust_events(
    sub_client: &SubscriptionClient,
    mut dust_wallet: DustWallet<DefaultDB>,
    start_id: i64,
    resuming: bool,
    checkpoint: Option<impl Fn(&DustWallet<DefaultDB>, i64)>,
    progress: Option<mpsc::Sender<SyncProgress>>,
) -> Result<DustReplay, WalletError> {
    use midnight_indexer_client::subscription::queries::DUST_LEDGER_EVENTS_SUBSCRIPTION;

    let mut later_ledger = None;

    let mut last_id: i64 = last_applied_before(start_id);
    let mut last_block_time: Option<Timestamp> = None;
    // Nullifiers of every DustSpendProcessed event seen during this replay,
    // surfaced to the caller so it can clear confirmed pending reservations.
    let mut spend_nullifiers: Vec<DustNullifier> = Vec::new();
    let mut count: u64 = 0;
    let mut since_checkpoint: u64 = 0;
    let mut retries: u32 = 0;
    // Semantic timeout above the client's transport keepalive; see
    // `replay_zswap_events` for the rationale.
    let event_timeout = if resuming {
        std::time::Duration::from_secs(10)
    } else {
        std::time::Duration::from_secs(30)
    };

    'reconnect: loop {
        let resume_id = resume_id(count, last_id);
        let variables = serde_json::json!({ "id": resume_id });
        let mut subscription = match sub_client
            .subscribe::<DustEventEnvelope>(DUST_LEDGER_EVENTS_SUBSCRIPTION, variables)
            .await
        {
            Ok(s) => s,
            Err(e) if e.is_retryable() && retries < RECONNECT_MAX_RETRIES => {
                retries += 1;
                warn!(retries, error = %e, "dust subscribe failed, retrying");
                tokio::time::sleep(reconnect_delay(retries)).await;
                continue 'reconnect;
            }
            Err(e) => return Err(gave_up("dust", e)),
        };
        // Highest id delivered on *this* connection; see `order_regression`.
        let mut conn_high: Option<i64> = None;

        loop {
            let event = tokio::time::timeout(event_timeout, subscription.next()).await;

            match event {
                Ok(Some(Ok(envelope))) => {
                    let msg = &envelope.dust_ledger_events;

                    if let Some(prev) = order_regression(msg.id, conn_high) {
                        return Err(WalletError::EventOrder {
                            kind: "dust",
                            id: msg.id,
                            prev,
                        });
                    }
                    conn_high = Some(msg.id);

                    if msg.max_id == 0 {
                        debug!("no dust events on this chain");
                        break 'reconnect;
                    }

                    if already_applied(msg.id, last_id, start_id, count > 0) {
                        debug!(id = msg.id, last_id, "skipping re-delivered dust event");
                        if msg.id >= msg.max_id {
                            info!(count, last_id, "dust replay complete");
                            send_progress(&progress, SyncProgress::DustComplete { events: count });
                            break 'reconnect;
                        }
                        continue;
                    }

                    let raw = event_bytes(msg, "dust")?;
                    let ledger = crate::LedgerVersion::of_event(&raw)?;
                    if ledger > super::LEDGER {
                        info!(id = msg.id, %ledger, "dust events move to a later ledger");
                        later_ledger = Some((msg.id, ledger));
                        break 'reconnect;
                    }
                    if ledger == super::LEDGER {
                        let ev = decode_event(&raw, "dust")?;
                        dust_wallet.replay_events([&ev]).map_err(|e| {
                            WalletError::Sync(format!("apply dust event id={}: {e}", msg.id))
                        })?;
                        if let Some(t) = event_block_time(&ev) {
                            last_block_time = Some(t);
                        }
                        if let Some(n) = event_spend_nullifier(&ev) {
                            spend_nullifiers.push(n);
                        }
                    }

                    // Only a new event counts as progress for the reconnect
                    // bound; deduped re-deliveries must not reset it.
                    retries = 0;
                    last_id = msg.id;
                    count += 1;
                    since_checkpoint += 1;

                    if count.is_multiple_of(10_000) {
                        info!(
                            count,
                            id = msg.id,
                            max_id = msg.max_id,
                            "dust replay progress"
                        );
                        let alive = send_progress(
                            &progress,
                            SyncProgress::DustEvents {
                                current: msg.id,
                                max: msg.max_id,
                            },
                        );
                        if !alive {
                            return Err(progress_cancelled("dust"));
                        }
                    }

                    if since_checkpoint >= DUST_CHECKPOINT_INTERVAL {
                        if let Some(ref save) = checkpoint {
                            save(&dust_wallet, last_id);
                        }
                        since_checkpoint = 0;
                    }

                    if msg.id >= msg.max_id {
                        info!(count, last_id, "dust replay complete");
                        send_progress(&progress, SyncProgress::DustComplete { events: count });
                        break 'reconnect;
                    }
                }
                Ok(Some(Err(e))) if e.is_retryable() && retries < RECONNECT_MAX_RETRIES => {
                    retries += 1;
                    warn!(retries, error = %e, "dust subscription dropped, reconnecting");
                    tokio::time::sleep(reconnect_delay(retries)).await;
                    continue 'reconnect;
                }
                Ok(Some(Err(e))) => return Err(gave_up("dust", e)),
                Ok(None) => {
                    if resuming && count == 0 {
                        info!(last_id, "dust already at tip");
                        send_progress(&progress, SyncProgress::DustComplete { events: 0 });
                        break 'reconnect;
                    }
                    // Mid-replay stream end without a `complete`: treat as a
                    // dropped connection and resume from the cursor.
                    if retries < RECONNECT_MAX_RETRIES {
                        retries += 1;
                        warn!(retries, "dust subscription ended early, reconnecting");
                        tokio::time::sleep(reconnect_delay(retries)).await;
                        continue 'reconnect;
                    }
                    return Err(WalletError::Sync(format!(
                        "dust subscription ended before replay completed \
                         (after {RECONNECT_MAX_RETRIES} reconnect attempts)"
                    )));
                }
                Err(_) => {
                    if resuming && count == 0 {
                        info!(last_id, "dust already at tip");
                        send_progress(&progress, SyncProgress::DustComplete { events: 0 });
                        break 'reconnect;
                    }
                    return Err(WalletError::Sync("timeout waiting for dust events".into()));
                }
            }
        }
    }

    Ok(DustReplay {
        wallet: dust_wallet,
        last_id,
        last_block_time,
        spend_nullifiers,
        later_ledger,
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

/// Decode a hex string into a 32-byte array. Returns `None` on hex decode
/// error or wrong length. Used to build typed hash wrappers
/// (`IntentHash`, `UnshieldedTokenType`, ...).
fn parse_hex_32(hex: &str) -> Option<[u8; 32]> {
    hex::decode(hex).ok()?.try_into().ok()
}

fn parse_token_type_hex(hex: &str) -> Option<UnshieldedTokenType> {
    parse_hex_32(hex).map(|arr| UnshieldedTokenType(HashOutput(arr)))
}

fn tracked_to_ledger_utxo(
    tracked: &TrackedUtxo,
    owner: helpers::UserAddress,
) -> Result<LedgerUtxo, WalletError> {
    let type_ = parse_token_type_hex(&tracked.token_type).ok_or_else(|| {
        WalletError::Sync(format!(
            "tracked UTXO has malformed token_type {}",
            tracked.token_type
        ))
    })?;
    let intent_hash_hex = tracked
        .intent_hash
        .as_deref()
        .ok_or_else(|| WalletError::Sync("tracked UTXO has no intent_hash".into()))?;
    let intent_hash = midnight_types::parse_intent_hash_hex(intent_hash_hex)
        .map(helpers::IntentHash)
        .ok_or_else(|| {
            WalletError::Sync(format!(
                "tracked UTXO has malformed intent_hash {intent_hash_hex}"
            ))
        })?;
    let idx = tracked
        .output_index
        .ok_or_else(|| WalletError::Sync("tracked UTXO has no output_index".into()))?;
    let output_no = u32::try_from(idx)
        .map_err(|_| WalletError::Sync(format!("tracked UTXO output_index {idx} out of range")))?;
    Ok(LedgerUtxo {
        value: tracked.value,
        owner,
        type_,
        intent_hash,
        output_no,
    })
}

#[cfg(test)]
mod tests {
    use helpers::coin_structure::coin::Commitment;
    use helpers::midnight_serialize::tagged_serialize;
    use helpers::mn_ledger::dust::{
        DustCommitment, DustPublicKey, InitialNonce, QualifiedDustOutput,
    };
    use helpers::mn_ledger::events::EventSource;
    use helpers::{
        DustLocalState, DustNullifier, DustSpend, Fr, HashOutput, INITIAL_PARAMETERS, KeyLocation,
        Nonce, Nullifier, ProofPreimage, ProofPreimageMarker, QualifiedInfo, Recipient,
        ShieldedTokenType, TransactionHash,
    };

    use super::super::types::DustSpendBatch;
    use super::*;

    #[test]
    fn anchor_window_clamps_to_the_tighter_dust_grace_period() {
        let global_ttl = helpers::Duration::from_secs(14 * 24 * 60 * 60); // 14 days
        let dust_grace = helpers::Duration::from_secs(3 * 60 * 60); // 3 hours
        // The dust grace window (the bound the node actually enforces against
        // `ctime`) is far shorter than the intent `global_ttl`, so it must win.
        assert_eq!(
            anchor_window(global_ttl, dust_grace).as_seconds(),
            3 * 60 * 60
        );
        // Symmetric: when `global_ttl` is the shorter of the two, it wins.
        assert_eq!(
            anchor_window(dust_grace, global_ttl).as_seconds(),
            3 * 60 * 60
        );
    }

    fn dust_event(content: EventDetails<DefaultDB>) -> Event<DefaultDB> {
        Event {
            source: EventSource {
                transaction_hash: TransactionHash(HashOutput([0u8; 32])),
                logical_segment: 0,
                physical_segment: 0,
            },
            content,
        }
    }

    #[test]
    fn event_spend_nullifier_matches_dust_spend_processed_only() {
        let nullifier = DustNullifier(Fr::from(7u64));
        let spend = dust_event(EventDetails::DustSpendProcessed {
            commitment: DustCommitment(Fr::from(8u64)),
            commitment_index: 0,
            nullifier,
            v_fee: 1,
            declared_time: Timestamp::from_secs(0),
            block_time: Timestamp::from_secs(0),
        });
        assert_eq!(event_spend_nullifier(&spend), Some(nullifier));

        let other = dust_event(EventDetails::ZswapInput {
            nullifier: Nullifier(HashOutput([1u8; 32])),
            contract: None,
        });
        assert_eq!(event_spend_nullifier(&other), None);
    }

    /// Minimal offline wallet for unit tests: fresh state, no sync.
    fn test_wallet(storage_dir: Option<PathBuf>) -> Wallet {
        let seed = WalletSeed::try_from_hex_str(&"22".repeat(32)).unwrap();
        let shielded = ShieldedWallet::<DefaultDB>::default(seed.clone());
        let secret_keys = shielded.secret_keys().clone();
        Wallet {
            seed: seed.clone(),
            secret_keys,
            network_id: "undeployed".into(),
            unshielded_address: "mn_addr_undeployed1test".into(),
            zswap_state: shielded.state.clone(),
            zswap_event_id: 0,
            dust_wallet: DustWallet::default(seed, Some(&INITIAL_PARAMETERS)),
            dust_event_id: 0,
            unshielded_utxos: Vec::new(),
            last_block_height: 0,
            last_tx_id: None,
            chain_pin: None,
            parameters: INITIAL_PARAMETERS,
            block_context: None,
            pending: PendingReservations::default(),
            storage_dir,
            indexer_url: "http://indexer.invalid".into(),
        }
    }

    /// A structurally-valid `DustSpend` whose identity is `DustNullifier(n)`.
    /// The proof is a placeholder preimage — the pending-replay paths only
    /// look at `old_nullifier`.
    fn dust_spend(n: u64) -> DustSpend<ProofPreimageMarker, DefaultDB> {
        DustSpend {
            v_fee: 1,
            old_nullifier: DustNullifier(Fr::from(n)),
            new_commitment: DustCommitment(Fr::from(n + 1)),
            proof: ProofPreimage {
                inputs: Vec::new(),
                private_transcript: Vec::new(),
                public_transcript_inputs: Vec::new(),
                public_transcript_outputs: Vec::new(),
                binding_input: Fr::from(0u64),
                communications_commitment: None,
                key_location: KeyLocation(std::borrow::Cow::Borrowed("test")),
            },
        }
    }

    fn dust_batch(nullifiers: &[u64]) -> DustSpendBatch {
        DustSpendBatch {
            seed: WalletSeed::try_from_hex_str(&"22".repeat(32)).unwrap(),
            spends: nullifiers.iter().map(|&n| dust_spend(n)).collect(),
            updated_state: Sp::new(DustLocalState::new(INITIAL_PARAMETERS.dust)),
        }
    }

    fn block_with_params(ledger_parameters: Option<String>) -> midnight_indexer_client::Block {
        midnight_indexer_client::Block {
            hash: "00".repeat(32),
            height: 1,
            protocol_version: None,
            timestamp: Some(1_000),
            author: None,
            transactions: None,
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

    #[test]
    fn build_context_refuses_pending_dust_without_dust_state() {
        let mut wallet = test_wallet(None);
        wallet.dust_wallet.dust_local_state = None;
        wallet.pending.reserve(
            vec![dust_batch(&[7])],
            Vec::new(),
            Vec::new(),
            Timestamp::from_secs(100),
        );

        let err = match wallet.build_context_inner() {
            Err(e) => e,
            Ok(_) => panic!("expected build_context_inner to refuse"),
        };
        assert!(matches!(err, WalletError::Transfer(_)));
        assert!(err.to_string().contains("pending dust reservation"));
    }

    #[test]
    fn build_context_allows_missing_dust_state_with_no_pending_dust() {
        // The register-dust bootstrap: no dust state yet, nothing pending.
        let mut wallet = test_wallet(None);
        wallet.dust_wallet.dust_local_state = None;
        assert!(wallet.build_context_inner().is_ok());
    }

    #[test]
    fn build_context_allows_pending_dust_when_state_present() {
        let mut wallet = test_wallet(None);
        wallet.pending.reserve(
            vec![dust_batch(&[7])],
            Vec::new(),
            Vec::new(),
            Timestamp::from_secs(100),
        );
        assert!(wallet.build_context_inner().is_ok());
    }

    /// A build evicts an expired reservation before it selects, so the
    /// balance read must not count one as reserved either.
    #[test]
    fn reserved_dust_nullifiers_skip_an_expired_reservation() {
        let mut wallet = test_wallet(None);
        let now = Timestamp::from_secs(100_000);
        wallet.block_context = Some(block_context_at(now));
        let expired = now - wallet.parameters.global_ttl - helpers::Duration::from_secs(1);
        wallet.reserve_pending(vec![dust_batch(&[1])], Vec::new(), Vec::new(), expired);
        wallet.reserve_pending(vec![dust_batch(&[2])], Vec::new(), Vec::new(), now);

        let reserved: Vec<DustNullifier> = wallet.reserved_dust_nullifiers().copied().collect();
        assert_eq!(reserved, vec![DustNullifier(Fr::from(2u64))]);
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
        );
        assert!(wallet.spendable_shielded_coins().is_empty());

        // And it is gone from the coin set the build context exposes (this is
        // the `coins.remove(nullifier)` path in build_context_inner).
        let ctx = wallet.build_context_inner().expect("build context");
        let wallets = ctx.wallets.lock().expect("wallets lock");
        let ctx_wallet = wallets
            .get(wallet.seed())
            .expect("funding wallet in context");
        assert_eq!(
            ctx_wallet.shielded.state.coins.iter().count(),
            0,
            "reserved coin must be removed from the build context's Zswap state"
        );
    }

    /// A NIGHT UTXO with both identity fields, so `tracked_to_ledger_utxo`
    /// accepts it.
    fn tracked_night_utxo() -> TrackedUtxo {
        TrackedUtxo {
            owner: "mn_addr_undeployed1test".into(),
            token_type: "00".repeat(32),
            value: 42,
            intent_hash: Some("ab".repeat(32)),
            output_index: Some(0),
            ctime: Some(0),
            registered_for_dust_generation: Some(false),
        }
    }

    #[test]
    fn execution_context_holds_no_wallet_and_no_utxos() {
        let mut wallet = test_wallet(None);
        wallet.unshielded_utxos = vec![tracked_night_utxo()];

        let ctx = wallet.execution_context().expect("execution context");

        assert!(
            ctx.wallets.lock().expect("wallets lock").is_empty(),
            "the execution half must carry no wallet, so it holds no seed or secret keys"
        );
        assert_eq!(
            ctx.with_ledger_state(|s| s.utxo.utxos.iter().count()),
            0,
            "the execution half must carry none of the wallet's UTXOs"
        );
    }

    #[test]
    fn add_funding_puts_the_wallet_and_its_utxos_in() {
        let mut wallet = test_wallet(None);
        wallet.unshielded_utxos = vec![tracked_night_utxo()];

        let ctx = wallet.execution_context().expect("execution context");
        wallet.add_funding(&ctx).expect("add funding");

        assert!(
            ctx.wallets
                .lock()
                .expect("wallets lock")
                .contains_key(wallet.seed()),
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
            vec![SpentUtxoKey {
                intent_hash: "ab".repeat(32),
                output_index: 0,
            }],
            Vec::new(),
            Timestamp::from_secs(100),
        );

        let ctx = wallet.execution_context().expect("execution context");
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
        let ctx = wallet.execution_context().expect("execution context");
        wallet.add_funding(&ctx).expect("first funding");

        // A second pass would merge into the first one's UTXO set, leaving an
        // input a pending build reserved still selectable.
        assert!(
            wallet.add_funding(&ctx).is_err(),
            "funding the same context twice must be refused"
        );
    }

    #[test]
    fn add_funding_reanchors_the_block_context() {
        let mut wallet = test_wallet(None);
        let ctx = wallet.execution_context().expect("execution context");
        assert!(ctx.latest_block_context.lock().unwrap().is_none());

        // A resync between the two halves moves the wallet's chain view. The
        // funding pass must carry it over, or the build witnesses a Dust root
        // the context's `ctime` does not resolve to.
        wallet.block_context = Some(block_context_at(Timestamp::from_secs(1_234)));
        wallet.add_funding(&ctx).expect("add funding");

        assert_eq!(
            ctx.latest_block_context
                .lock()
                .unwrap()
                .as_ref()
                .map(|bc| bc.tblock),
            Some(Timestamp::from_secs(1_234)),
            "funding must re-anchor the context on its own chain view"
        );
    }

    #[test]
    fn reserve_pending_persists_pending_file_when_storage_dir_set() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut wallet = test_wallet(Some(dir.path().to_path_buf()));
        wallet.reserve_pending(
            Vec::new(),
            vec![SpentUtxoKey {
                intent_hash: "abcd".into(),
                output_index: 0,
            }],
            Vec::new(),
            Timestamp::from_secs(100),
        );

        let loaded = PendingReservations::load(dir.path(), "undeployed", &wallet.storage_id())
            .unwrap()
            .expect("pending.json should exist after reserve_pending");
        assert_eq!(loaded.unshielded_keys().count(), 1);
    }

    /// A coin the wallet owns and can rebuild, but whose output carries
    /// nothing it can decrypt.
    fn unreachable_coin(byte: u8, value: u128) -> crate::CoinInfo {
        crate::CoinInfo {
            nonce: crate::Nonce(HashOutput([byte; 32])),
            type_: crate::ShieldedTokenType(HashOutput([3u8; 32])),
            value,
        }
    }

    /// The commitment `coin`'s output carries when `wallet` owns it.
    fn commitment_of(wallet: &Wallet, coin: &crate::CoinInfo) -> Commitment {
        coin.into_ledger()
            .commitment(&Recipient::User(wallet.secret_keys.coin_public_key()))
    }

    #[test]
    fn watch_for_coin_records_the_commitment_the_output_carries() {
        let mut wallet = test_wallet(None);
        let coin = unreachable_coin(9, 42);

        wallet.watch_for_coin(coin).unwrap();

        assert!(
            wallet
                .zswap_state()
                .pending_outputs
                .contains_key(&commitment_of(&wallet, &coin)),
            "registration must key on the commitment a replay meets on chain"
        );
        assert_eq!(wallet.watched_coins(), vec![coin]);
    }

    /// The order the ledger's map iterates in is deterministic but
    /// unspecified, so the accessor sorts. Callers that compare or display
    /// the list need an order they can rely on.
    #[test]
    fn watched_coins_returns_a_stable_order() {
        let mut wallet = test_wallet(None);
        let coins = [
            unreachable_coin(9, 42),
            unreachable_coin(1, 7),
            unreachable_coin(5, 100),
        ];
        wallet.watch_for_coins(coins).unwrap();

        let mut expected = coins;
        expected.sort_unstable();
        assert_eq!(wallet.watched_coins(), expected.to_vec());
    }

    /// A registration that matches no on-chain output would otherwise sit in
    /// the watched set and ride along on every later replay.
    #[test]
    fn forget_coin_drops_a_registration() {
        let mut wallet = test_wallet(None);
        let wrong = unreachable_coin(9, 41);
        let right = unreachable_coin(9, 42);
        wallet.watch_for_coins([wrong, right]).unwrap();

        wallet.forget_coin(wrong).unwrap();

        assert_eq!(wallet.watched_coins(), vec![right]);
        assert!(!wallet.coins_to_register().contains(&wrong.into_ledger()));
    }

    /// Forgetting is about registrations, not coins. A claimed coin has no
    /// registration left to drop, and must stay spendable.
    #[test]
    fn forget_coin_leaves_a_claimed_coin_alone() {
        let mut wallet = test_wallet(None);
        let coin = unreachable_coin(9, 42);
        wallet.zswap_state = wallet
            .zswap_state
            .insert_coin(&wallet.secret_keys, coin.into_ledger())
            .expect("claim the coin");

        wallet.forget_coin(coin).unwrap();

        assert_eq!(wallet.spendable_shielded_coins().len(), 1);
    }

    /// Registering or forgetting nothing must not rewrite the multi-MB
    /// generation files.
    #[test]
    fn registering_nothing_writes_nothing() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut wallet = test_wallet(Some(dir.path().to_path_buf()));
        wallet.save(dir.path()).unwrap();

        wallet.watch_for_coins([]).unwrap();
        wallet.forget_coin(unreachable_coin(9, 42)).unwrap();

        assert_eq!(stored_generations(dir.path()), vec![1]);
    }

    /// A coin registered before it lands on chain has to survive a restart,
    /// so the registration is part of the persisted wallet state.
    #[test]
    fn watch_for_coin_persists_the_registration() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut wallet = test_wallet(Some(dir.path().to_path_buf()));
        let coin = unreachable_coin(9, 42);

        wallet.watch_for_coin(coin).unwrap();

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

    /// The plan replays from nothing: the events rebuild every coin the
    /// wallet held, and the registrations ride along so the replay can claim
    /// the outputs it could not decrypt.
    #[test]
    fn shielded_rescan_plan_starts_empty_and_carries_the_registrations() {
        let mut wallet = test_wallet(None);
        insert_shielded_coin(&mut wallet, 7, 100);
        let coin = unreachable_coin(9, 42);
        wallet.watch_for_coin(coin).unwrap();

        let plan = wallet.shielded_rescan_plan();

        assert_eq!(plan.initial_state.first_free, 0);
        assert!(plan.initial_state.coins.is_empty());
        assert!(
            plan.initial_state
                .pending_outputs
                .contains_key(&commitment_of(&wallet, &coin))
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
        assert_eq!(wallet.zswap_event_id(), 9);
        assert_eq!(wallet.dust_event_id(), 11, "a rescan is shielded-only");
        assert_eq!(stored_generations(dir.path()), vec![1]);
    }

    /// A rescan's replay starts before the caller can register anything, so a
    /// registration made while it runs is missing from the state it commits.
    #[test]
    fn commit_shielded_rescan_carries_a_registration_made_during_the_replay() {
        let mut wallet = test_wallet(None);
        let coin = unreachable_coin(9, 42);
        wallet.watch_for_coin(coin).unwrap();

        wallet
            .commit_shielded_rescan(ShieldedRescanCommit {
                zswap_state: ZswapLocalState::new(),
                zswap_event_id: 9,
            })
            .unwrap();

        assert_eq!(wallet.watched_coins(), vec![coin]);
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
            last_block_height: 0,
            spent_unshielded: Vec::new(),
            chain_tblock: Timestamp::from_secs(1_000),
            parameters: wallet.parameters.clone(),
        }
    }

    #[test]
    fn noop_resync_commit_skips_persistence() {
        // Seam for the resync commit path: resync runs before every build,
        // so a commit that changes no durable state must not rewrite the
        // generation files, even though it refreshes `block_context`.
        let dir = tempfile::TempDir::new().unwrap();
        let mut wallet = test_wallet(Some(dir.path().to_path_buf()));
        wallet.last_tx_id = Some(3);
        wallet.save(dir.path()).unwrap();
        assert_eq!(stored_generations(dir.path()), vec![1]);

        let commit = noop_commit(&wallet);
        wallet.commit_resync(commit).unwrap();

        assert_eq!(stored_generations(dir.path()), vec![1]);
        // The non-durable block context was still refreshed.
        assert!(wallet.block_context.is_some());
    }

    #[test]
    fn resync_commit_persists_when_cursor_advances() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut wallet = test_wallet(Some(dir.path().to_path_buf()));
        wallet.last_tx_id = Some(3);
        wallet.save(dir.path()).unwrap();

        let mut commit = noop_commit(&wallet);
        commit.dust_event_id += 1;
        wallet.commit_resync(commit).unwrap();

        assert_eq!(wallet.dust_event_id, 1);
        assert_eq!(stored_generations(dir.path()), vec![2]);
    }

    #[test]
    fn resync_commit_persists_when_parameters_change() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut wallet = test_wallet(Some(dir.path().to_path_buf()));
        wallet.last_tx_id = Some(3);
        wallet.save(dir.path()).unwrap();

        let mut commit = noop_commit(&wallet);
        commit
            .parameters
            .cardano_to_midnight_bridge_fee_basis_points += 1;
        wallet.commit_resync(commit).unwrap();

        assert_eq!(stored_generations(dir.path()), vec![2]);
    }

    #[test]
    fn resync_commit_persists_when_reservation_cleared() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut wallet = test_wallet(Some(dir.path().to_path_buf()));
        wallet.last_tx_id = Some(3);
        wallet.save(dir.path()).unwrap();
        let key = SpentUtxoKey {
            intent_hash: "abcd".into(),
            output_index: 0,
        };
        wallet.reserve_pending(
            Vec::new(),
            vec![key.clone()],
            Vec::new(),
            Timestamp::from_secs(100),
        );

        let mut commit = noop_commit(&wallet);
        commit.spent_unshielded = vec![key];
        wallet.commit_resync(commit).unwrap();

        assert!(wallet.pending.is_empty());
        assert_eq!(stored_generations(dir.path()), vec![2]);
        assert!(
            PendingReservations::load(dir.path(), "undeployed", &wallet.storage_id())
                .unwrap()
                .is_none()
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
        let confirmed = SpentUtxoKey {
            intent_hash: "aaaa".into(),
            output_index: 0,
        };
        wallet.reserve_pending(
            Vec::new(),
            vec![confirmed.clone()],
            Vec::new(),
            Timestamp::from_secs(100),
        );

        // Snapshot the replay inputs (the plan itself must not mutate).
        let _plan = wallet.resync_plan();

        // A transfer build interleaves while the (conceptual) replay runs.
        let late = SpentUtxoKey {
            intent_hash: "bbbb".into(),
            output_index: 1,
        };
        wallet.reserve_pending(
            Vec::new(),
            vec![late.clone()],
            Vec::new(),
            Timestamp::from_secs(101),
        );

        // The replay observed only the first reservation's spend.
        let mut commit = noop_commit(&wallet);
        commit.spent_unshielded = vec![confirmed];
        wallet.commit_resync(commit).unwrap();

        let remaining: Vec<_> = wallet.pending.unshielded_keys().cloned().collect();
        assert_eq!(remaining, vec![late], "late reservation must survive");
    }

    /// A resync does not end a shielded reservation when the spend confirms,
    /// so a check of the reservation never sees a shielded spend.
    #[test]
    fn has_observed_reads_the_coin_set_not_the_shielded_reservation() {
        let mut wallet = test_wallet(None);
        let nullifier = insert_shielded_coin(&mut wallet, 7, 100);
        let reserved_at = Timestamp::from_secs(100);
        wallet.reserve_pending(Vec::new(), Vec::new(), vec![nullifier], reserved_at);
        let spent = [crate::SpentInputs::from_shielded(
            vec![nullifier.into_sdk()],
            reserved_at,
        )];
        assert!(!wallet.has_observed(&spent));

        let mut commit = noop_commit(&wallet);
        commit.zswap_state.coins = commit.zswap_state.coins.remove(&nullifier);
        wallet.commit_resync(commit).unwrap();

        assert!(wallet.has_observed(&spent));
        assert!(
            wallet
                .reserved_shielded_nullifiers()
                .any(|n| *n == nullifier),
            "the reservation must still stand, or this proves nothing about it"
        );
    }

    /// A release ends an unshielded reservation with no spend on chain, so a
    /// check of the reservation reports a spend that the wallet never saw.
    #[test]
    fn has_observed_reads_the_unshielded_utxos_not_the_reservation() {
        let mut wallet = test_wallet(None);
        wallet.unshielded_utxos = vec![tracked_night_utxo()];
        let key = SpentUtxoKey {
            intent_hash: "ab".repeat(32),
            output_index: 0,
        };
        let reserved_at = Timestamp::from_secs(100);
        wallet.reserve_pending(Vec::new(), vec![key.clone()], Vec::new(), reserved_at);
        let spent = [crate::SpentInputs {
            unshielded: vec![key],
            reserved_at,
            ..Default::default()
        }];
        wallet.release(&spent[0]);
        assert_eq!(
            wallet.pending.unshielded_keys().count(),
            0,
            "the release must end the reservation, or this proves nothing about it"
        );

        assert!(!wallet.has_observed(&spent));

        wallet.unshielded_utxos.clear();
        assert!(wallet.has_observed(&spent));
    }

    /// A release ends a Dust reservation with no spend on chain, so a check of
    /// the reservation reports a spend that the wallet never saw.
    #[test]
    fn has_observed_reads_the_dust_state_not_the_reservation() {
        let mut wallet = test_wallet(None);
        let nullifier = DustNullifier(Fr::from(1u64));
        let utxo = QualifiedDustOutput {
            initial_value: 1,
            owner: DustPublicKey(Fr::from(0u64)),
            nonce: Fr::from(0u64),
            seq: 0,
            ctime: Timestamp::from_secs(0),
            backing_night: InitialNonce(HashOutput([0u8; 32])),
            mt_index: 0,
        };
        let state = DustLocalState::new(INITIAL_PARAMETERS.dust)
            .add_utxo(&nullifier, &utxo, None)
            .unwrap();
        wallet.dust_wallet.dust_local_state = Some(Sp::new(state.clone()));
        let reserved_at = Timestamp::from_secs(100);
        wallet.reserve_pending(vec![dust_batch(&[1])], Vec::new(), Vec::new(), reserved_at);
        let spent = [crate::SpentInputs {
            dust: vec![nullifier.into_sdk()],
            reserved_at,
            ..Default::default()
        }];
        wallet.release(&spent[0]);
        assert_eq!(
            wallet.reserved_dust_nullifiers().count(),
            0,
            "the release must end the reservation, or this proves nothing about it"
        );

        assert!(!wallet.has_observed(&spent));

        wallet.dust_wallet.dust_local_state = Some(Sp::new(state.remove_utxo(&nullifier).unwrap()));
        assert!(wallet.has_observed(&spent));
    }

    #[test]
    fn reserve_pending_keeps_reservation_when_persistence_fails() {
        // `storage_dir` points at a regular file, so `save_pending` cannot
        // create the wallet directory and the disk write fails. The write
        // is best-effort: no panic, and the in-memory reservation must
        // still gate `build_context_inner`.
        let dir = tempfile::TempDir::new().unwrap();
        let blocker = dir.path().join("not-a-directory");
        std::fs::write(&blocker, b"occupied").unwrap();

        let mut wallet = test_wallet(Some(blocker));
        wallet.dust_wallet.dust_local_state = None;
        wallet.reserve_pending(
            vec![dust_batch(&[7])],
            vec![SpentUtxoKey {
                intent_hash: "abcd".into(),
                output_index: 0,
            }],
            Vec::new(),
            Timestamp::from_secs(100),
        );

        assert_eq!(wallet.pending.unshielded_keys().count(), 1);
        assert_eq!(wallet.pending.dust_batches().count(), 1);
        // With no dust state to replay the pending batch against, the
        // surviving reservation still refuses the build.
        assert!(matches!(
            wallet.build_context_inner(),
            Err(WalletError::Transfer(_))
        ));
    }

    fn params_with(
        mutate: impl FnOnce(&mut helpers::LedgerParameters),
    ) -> helpers::LedgerParameters {
        let mut p = INITIAL_PARAMETERS;
        mutate(&mut p);
        p
    }

    #[test]
    fn validate_ledger_parameters_rejects_zeroed_fields() {
        use helpers::base_crypto::cost_model::FixedPoint;

        let cases = vec![
            (
                "global_ttl",
                params_with(|p| p.global_ttl = helpers::Duration::from_secs(0)),
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
        let corrupt = params_with(|p| p.global_ttl = helpers::Duration::from_secs(0));
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
        use helpers::mn_ledger::events::ZswapPreimageEvidence;
        use midnight_indexer_client::testutil::{accept_subscriber, bind, next_json, send_next};
        use serde_json::json;

        use super::*;

        /// The chain's event for an output whose recipient gets no discovery
        /// ciphertext: the commitment lands in the Merkle tree and nothing
        /// else identifies the coin.
        fn ciphertextless_output(commitment: Commitment, mt_index: u64) -> String {
            let event: Event<DefaultDB> = Event {
                source: EventSource {
                    transaction_hash: TransactionHash(HashOutput([0u8; 32])),
                    logical_segment: 0,
                    physical_segment: 0,
                },
                content: EventDetails::ZswapOutput {
                    commitment,
                    preimage_evidence: ZswapPreimageEvidence::None,
                    contract: None,
                    mt_index,
                },
            };
            let mut raw = Vec::new();
            tagged_serialize(&event, &mut raw).unwrap();
            hex::encode(raw)
        }

        fn zswap_message(id: i64, raw: &str, max_id: i64) -> serde_json::Value {
            json!({"zswapLedgerEvents": {"id": id, "raw": raw, "maxId": max_id}})
        }

        fn requested_zswap_id(sub: &serde_json::Value) -> i64 {
            sub["payload"]["variables"]["id"]
                .as_i64()
                .expect("id variable")
        }

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
            let raw = ciphertextless_output(commitment_of(&wallet, &coin), 0);

            let server = tokio::spawn(async move {
                // The same one-event stream twice: once for the replay that
                // stands in for the original sync, once for the rescan.
                for _ in 0..2 {
                    let (mut ws, sub) = accept_subscriber(&listener).await;
                    assert_eq!(
                        requested_zswap_id(&sub),
                        0,
                        "a rescan must replay from the first event, whatever the cursor says"
                    );
                    send_next(&mut ws, &sub, zswap_message(1, &raw, 1)).await;
                    while next_json(&mut ws).await.is_some() {}
                }
            });

            rescan(&mut wallet, &url).await;
            assert!(
                wallet.spendable_shielded_coins().is_empty(),
                "an unregistered coin with no ciphertext is not discoverable"
            );
            assert_eq!(wallet.zswap_event_id(), 1);

            wallet.watch_for_coin(coin).unwrap();
            rescan(&mut wallet, &url).await;

            let claimed = wallet.spendable_shielded_coins();
            assert_eq!(claimed.len(), 1, "the registered coin must be claimed");
            assert_eq!(claimed[0].value, 42);
            assert_eq!(claimed[0].nonce, [9u8; 32]);
            assert!(
                wallet.watched_coins().is_empty(),
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
                    &mut helpers::OsRng,
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
                ciphertextless_output(commitment_of(&wallet, &first), 0),
                ciphertextless_output(commitment_of(&wallet, &second), 1),
            ];

            let server = tokio::spawn(async move {
                // One two-output stream, replayed once per rescan.
                for _ in 0..2 {
                    let (mut ws, sub) = accept_subscriber(&listener).await;
                    assert_eq!(requested_zswap_id(&sub), 0);
                    for (index, raw) in stream.iter().enumerate() {
                        send_next(&mut ws, &sub, zswap_message(index as i64 + 1, raw, 2)).await;
                    }
                    while next_json(&mut ws).await.is_some() {}
                }
            });

            wallet.watch_for_coin(first).unwrap();
            rescan(&mut wallet, &url).await;
            assert_eq!(wallet.spendable_shielded_coins().len(), 1);

            // Registering the second coin replays again, which must not cost
            // us the first.
            wallet.watch_for_coin(second).unwrap();
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
    }

    /// A resync's plan is snapshotted before its replay and committed after,
    /// so a registration made in between is not in the state it commits.
    /// Dropping it would leave the caller with a coin nothing can claim and
    /// no record that it was ever registered.
    #[test]
    fn commit_resync_carries_a_registration_made_during_the_replay() {
        let mut wallet = test_wallet(None);
        let commit = noop_commit(&wallet);

        let coin = unreachable_coin(9, 42);
        wallet.watch_for_coin(coin).unwrap();
        wallet.commit_resync(commit).unwrap();

        assert_eq!(wallet.watched_coins(), vec![coin]);
    }

    /// A registration the replay claimed is not carried back: the coin is
    /// held now, and a stale pending output would make `watched_coins` report
    /// a coin the wallet already has.
    #[test]
    fn commit_resync_drops_a_registration_its_replay_claimed() {
        let mut wallet = test_wallet(None);
        let coin = unreachable_coin(9, 42);
        wallet.watch_for_coin(coin).unwrap();

        // The replay met the output and claimed it, so the committed state
        // holds the coin and no longer carries the registration.
        let mut commit = noop_commit(&wallet);
        commit.zswap_state = ZswapLocalState::new()
            .insert_coin(&wallet.secret_keys, coin.into_ledger())
            .expect("insert claimed coin");
        wallet.commit_resync(commit).unwrap();

        assert_eq!(wallet.spendable_shielded_coins().len(), 1);
        assert!(wallet.watched_coins().is_empty());
    }

    /// Mock-WebSocket-server tests for the zswap and dust replays' resume.
    mod resume_ws {
        use midnight_indexer_client::testutil::{accept_subscriber, bind, next_json, send_next};
        use serde_json::json;

        use super::*;

        fn ledger_event(stream: &str, id: i64, max_id: i64) -> serde_json::Value {
            json!({ stream: {"id": id, "maxId": max_id, "raw": "00"} })
        }

        fn requested_id(sub: &serde_json::Value) -> i64 {
            sub["payload"]["variables"]["id"]
                .as_i64()
                .expect("id variable")
        }

        /// A resume asks for the event it already applied, so the indexer
        /// answers at once and its `max_id` says the wallet is at the tip.
        ///
        /// Asking for the next event instead leaves the indexer with nothing
        /// to send, and the replay can only read that silence by waiting out
        /// its 10s timeout. Every build pays that, because
        /// `MidnightProvider::execution_context` resyncs first.
        #[tokio::test]
        async fn zswap_resume_at_the_tip_returns_without_waiting() {
            let (listener, url) = bind().await;
            let server = tokio::spawn(async move {
                let (mut ws, sub) = accept_subscriber(&listener).await;
                let asked = requested_id(&sub);
                send_next(&mut ws, &sub, ledger_event("zswapLedgerEvents", 7, 7)).await;
                while next_json(&mut ws).await.is_some() {}
                asked
            });

            let sub_client = SubscriptionClient::new(&url);
            let shielded = ShieldedWallet::<DefaultDB>::default(
                WalletSeed::try_from_hex_str(&"22".repeat(32)).unwrap(),
            );
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

        /// The dust replay carries its own copy of the resume, so it needs
        /// its own guard. See the zswap test for what the cost is.
        #[tokio::test]
        async fn dust_resume_at_the_tip_returns_without_waiting() {
            let (listener, url) = bind().await;
            let server = tokio::spawn(async move {
                let (mut ws, sub) = accept_subscriber(&listener).await;
                let asked = requested_id(&sub);
                send_next(&mut ws, &sub, ledger_event("dustLedgerEvents", 7, 7)).await;
                while next_json(&mut ws).await.is_some() {}
                asked
            });

            let sub_client = SubscriptionClient::new(&url);
            let started = std::time::Instant::now();
            let replay = replay_dust_events(
                &sub_client,
                DustWallet::default(
                    WalletSeed::try_from_hex_str(&"22".repeat(32)).unwrap(),
                    Some(&INITIAL_PARAMETERS),
                ),
                8,
                true,
                None::<fn(&DustWallet<DefaultDB>, i64)>,
                None,
            )
            .await
            .expect("a resume at the tip must succeed");

            assert_eq!(
                replay.last_id, 7,
                "the re-delivered event must not advance the cursor"
            );
            assert!(
                replay.spend_nullifiers.is_empty(),
                "a skipped event must apply nothing"
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
