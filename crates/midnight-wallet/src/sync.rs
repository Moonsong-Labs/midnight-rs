//! Build a synced [`Wallet`] from its seed or mnemonic, as a standalone step.

use std::path::{Path, PathBuf};

use midnight_types::chain_pin::{
    ChainCheck, ChainPin, ChainView, SyncSource, current_pin, verify_pin,
};
use midnight_types::{Network, WalletError};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::warn;

use crate::{Seed, SeedError, Wallet};

/// Progress updates emitted during wallet sync.
#[derive(Debug, Clone)]
pub enum SyncProgress {
    /// The sync resumes from a stored snapshot.
    Resuming {
        /// The id of the last zswap event that the snapshot applied.
        zswap_event_id: i64,
        /// The id of the last Dust event that the snapshot read.
        dust_event_id: i64,
    },
    /// The zswap replay applied another 10,000 events.
    ZswapEvents {
        /// The id of the last zswap event that the replay applied.
        current: i64,
        /// The highest zswap event id that the indexer reports.
        max: i64,
    },
    /// The zswap replay reached the highest event id that the indexer
    /// reports.
    ZswapComplete {
        /// The number of zswap events that the replay applied after the last
        /// hard fork that it crossed, or all of them when it crossed none. It
        /// is 0 when the wallet was at the tip.
        events: u64,
    },
    /// The Dust replay read another 10,000 new events.
    DustEvents {
        /// The id of the last Dust event that the replay read.
        current: i64,
        /// The highest Dust event id that the indexer reports.
        max: i64,
    },
    /// The Dust replay reached the highest event id that the indexer reports.
    DustComplete {
        /// The number of new Dust events that the replay read, 0 when the
        /// wallet was at the tip. It includes the events of an earlier ledger
        /// generation, which the replay skips.
        events: u64,
    },
    /// The unshielded replay reached the last transaction that the indexer
    /// reports for the wallet's address.
    UnshieldedCaughtUp {
        /// The number of unspent UTXOs that the wallet holds at its address.
        utxos: usize,
    },
}

impl Wallet {
    /// Start a wallet from its seed, written as hex.
    ///
    /// The hex decodes to 16, 32 or 64 bytes. The dev seed that the local
    /// devnet funds at genesis has this form.
    ///
    /// ```rust,no_run
    /// # use midnight_provider::MidnightProvider;
    /// # use midnight_wallet::Wallet;
    /// # async fn example(provider: MidnightProvider) -> Result<(), Box<dyn std::error::Error>> {
    /// let wallet = Wallet::from_seed("0000000000000000000000000000000000000000000000000000000000000001")?
    ///     .sync(&provider)
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// [`SeedError::InvalidHex`] when `seed` is not hex, and
    /// [`SeedError::InvalidLength`] when it decodes to another length.
    pub fn from_seed(seed: &str) -> Result<WalletBuilder, SeedError> {
        Seed::from_hex(seed).map(WalletBuilder::new)
    }

    /// Start a wallet from a BIP-39 mnemonic phrase, with an empty
    /// passphrase.
    ///
    /// The phrase has 12, 15, 18, 21 or 24 English words. The wallet uses the
    /// 64-byte seed of the standard BIP-39 derivation.
    ///
    /// ```rust,no_run
    /// # use midnight_provider::MidnightProvider;
    /// # use midnight_wallet::Wallet;
    /// # async fn example(provider: MidnightProvider, phrase: &str) -> Result<(), Box<dyn std::error::Error>> {
    /// let wallet = Wallet::from_mnemonic(phrase)?.sync(&provider).await?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// [`SeedError::InvalidMnemonic`] when the phrase is not a valid BIP-39
    /// mnemonic.
    pub fn from_mnemonic(phrase: &str) -> Result<WalletBuilder, SeedError> {
        Seed::from_mnemonic(phrase).map(WalletBuilder::new)
    }

    /// Start a wallet from a BIP-39 mnemonic phrase and its passphrase (the
    /// "25th word").
    ///
    /// Each passphrase gives a different wallet for the same phrase.
    ///
    /// # Errors
    ///
    /// [`SeedError::InvalidMnemonic`] when the phrase is not a valid BIP-39
    /// mnemonic.
    pub fn from_mnemonic_with_passphrase(
        phrase: &str,
        passphrase: &str,
    ) -> Result<WalletBuilder, SeedError> {
        Seed::from_mnemonic_with_passphrase(phrase, passphrase).map(WalletBuilder::new)
    }
}

/// Handle to the background task spawned by
/// [`WalletBuilder::sync_with_progress`].
///
/// Awaiting it yields the synced [`Wallet`], so a single `?` is enough. A
/// panic or cancellation of the spawned task surfaces as
/// [`WalletError::SyncTaskJoin`]; the inner sync error path surfaces as the
/// matching [`WalletError`] variant.
///
/// **Dropping the handle cancels the sync.** The handle is the only way to
/// obtain the synced wallet, so once it is dropped the sync's result is
/// unobservable and letting it run would only keep three indexer WebSocket
/// subscriptions alive for nothing. To run a sync without holding a
/// `SyncHandle`, spawn [`WalletBuilder::sync`] yourself. It borrows its
/// source, so move an `Arc` of the source into the task:
///
/// ```rust,no_run
/// # use std::sync::Arc;
/// # use midnight_provider::MidnightProvider;
/// # use midnight_wallet::Wallet;
/// # async fn example(provider: Arc<MidnightProvider>) -> Result<(), Box<dyn std::error::Error>> {
/// let builder = Wallet::from_seed("0000000000000000000000000000000000000000000000000000000000000001")?;
/// let source = Arc::clone(&provider);
/// let task = tokio::spawn(async move { builder.sync(&*source).await });
/// let wallet = task.await??;
/// # Ok(())
/// # }
/// ```
pub struct SyncHandle {
    inner: JoinHandle<Result<Wallet, WalletError>>,
}

impl std::fmt::Debug for SyncHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SyncHandle")
            .field("finished", &self.inner.is_finished())
            .finish_non_exhaustive()
    }
}

impl Drop for SyncHandle {
    fn drop(&mut self) {
        // Cancel-on-drop (see the struct docs). Aborting the task drops its
        // in-flight `Subscription` handles, which tear down their WebSocket
        // reader tasks — no orphaned subscriptions survive the handle. The
        // sync task holds no locks at any await point, so an abort cannot
        // strand one. No-op if the task already finished, e.g. after the
        // handle was awaited to completion.
        self.inner.abort();
    }
}

impl std::future::Future for SyncHandle {
    type Output = Result<Wallet, WalletError>;

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        std::pin::Pin::new(&mut self.inner)
            .poll(cx)
            .map(|outer| match outer {
                Ok(inner) => inner,
                Err(join_err) => Err(WalletError::SyncTaskJoin(join_err.to_string())),
            })
    }
}

/// A wallet before its sync: its seed, and the options of the sync.
///
/// [`Wallet::from_seed`], [`Wallet::from_mnemonic`],
/// [`Wallet::from_mnemonic_with_passphrase`] and [`Self::new`] start one. It
/// holds no connection and does no work until a sync step runs:
///
/// - [`sync`](Self::sync) syncs in the current task and returns the synced
///   [`Wallet`].
/// - [`sync_with_progress`](Self::sync_with_progress) syncs in a background
///   task. It returns a receiver of [`SyncProgress`] events and a
///   [`SyncHandle`] that resolves to the synced wallet.
///
/// A sync takes its source as a `&dyn` [`SyncSource`], so the builder names
/// no provider. Attach the synced wallet afterwards:
///
/// ```rust,no_run
/// # use midnight_provider::MidnightProvider;
/// # use midnight_wallet::Wallet;
/// # const NODE_URL: &str = "ws://localhost:9944";
/// # const INDEXER_URL: &str = "http://localhost:8088";
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let provider = MidnightProvider::new(NODE_URL, INDEXER_URL);
/// let wallet = Wallet::from_seed("0000000000000000000000000000000000000000000000000000000000000001")?
///     .sync(&provider)
///     .await?;
/// let provider = provider.with_wallet(wallet);
/// # Ok(())
/// # }
/// ```
///
/// Every sync pins the wallet to the chain by default, as [`Self::sync`]
/// describes.
#[derive(Debug)]
#[must_use = "a wallet builder does nothing until it syncs"]
pub struct WalletBuilder {
    seed: Seed,
    network: Option<Network>,
    storage_dir: Option<PathBuf>,
    pin: bool,
}

/// The owned inputs a sync runs on, once the chain work is done. Holding no
/// borrow is what lets [`WalletBuilder::sync_with_progress`] spawn.
pub(crate) struct SyncPlan {
    pub(crate) indexer_url: String,
    pub(crate) seed: Seed,
    pub(crate) address: String,
    pub(crate) network: Network,
    pub(crate) storage_dir: Option<PathBuf>,
    /// The finalized block the node reports now, persisted so a later
    /// resume can ask whether it is still on this chain. `None` when the
    /// sync is unpinned or the node could not answer.
    pub(crate) chain_pin: Option<ChainPin>,
}

impl WalletBuilder {
    /// Start a wallet from a [`Seed`].
    ///
    /// A `&Seed` also works, and leaves the `Seed` to the caller. For a seed
    /// written as hex or a mnemonic phrase, use [`Wallet::from_seed`] or
    /// [`Wallet::from_mnemonic`].
    pub fn new(seed: impl Into<Seed>) -> Self {
        Self {
            seed: seed.into(),
            network: None,
            storage_dir: None,
            pin: true,
        }
    }

    /// Sync as `network`, and refuse a node that runs another network.
    ///
    /// Without this call, the wallet takes the network that the node
    /// reports. Name the network to guard against a node URL of the wrong
    /// network. A pinned sync (the default) then fails with
    /// [`WalletError::NetworkMismatch`] before it reads the indexer. A node
    /// that cannot answer, reports no network, or reports an id that is not
    /// a valid name for a [`Network`] does not fail the sync.
    ///
    /// For a name from configuration, parse it first with
    /// `name.parse::<Network>()?`.
    pub fn with_network(mut self, network: impl Into<Network>) -> Self {
        self.network = Some(network.into());
        self
    }

    /// Persist sync progress + recovered state under `dir`. Without this call,
    /// the wallet runs in-memory only. The directory is retained: every
    /// successful resync re-saves the wallet and each transfer build
    /// persists its pending reservation.
    ///
    /// See [`docs/wallet.md`](https://github.com/Moonsong-Labs/midnight-rs/blob/main/docs/wallet.md#persistence)
    /// for the on-disk layout.
    pub fn with_storage(mut self, dir: impl Into<PathBuf>) -> Self {
        self.storage_dir = Some(dir.into());
        self
    }

    /// Skip the network check and the pin at sync time.
    ///
    /// The sync then sends the node only one query: the network it runs,
    /// and only when [`with_network`](Self::with_network) names none. The
    /// sync still keeps a stored snapshot's pin, and every later resync
    /// checks that pin. A wallet synced unpinned with no stored pin stays
    /// unpinned for its life. [`Self::sync`] describes the chain-reset guard.
    pub fn unpinned(mut self) -> Self {
        self.pin = false;
        self
    }

    /// Sync the wallet from `source` in the current task.
    ///
    /// The sync starts from genesis, or from a stored snapshot. It replays the
    /// source's indexer, and the wallet keeps that indexer for its later
    /// resyncs. `MidnightProvider` is a source.
    ///
    /// Before the replay, the sync asks the source's node which network it
    /// runs. A wallet with no network takes that network. A wallet with a
    /// network fails the sync with [`WalletError::NetworkMismatch`] when the
    /// node runs another one. A node that reports no network, or an id that
    /// is not a valid name for a [`Network`], fails the sync only when the
    /// wallet has no network.
    ///
    /// The source's node also pins the wallet to the chain. This pin is the
    /// chain-reset guard. A snapshot's cursors are counts, so a snapshot
    /// from a replaced chain resumes cleanly and reports the dead chain's
    /// balance. To catch this, the sync checks a stored snapshot's pin
    /// against the node before the replay starts. A pin that the chain no
    /// longer holds fails the sync with [`WalletError::ChainMismatch`].
    ///
    /// A node that cannot answer does not fail the pin check, because a
    /// pruned archive must not condemn a healthy wallet. The synced wallet
    /// also carries a fresh pin, which every resync checks again. Thus the
    /// guard holds for the wallet's whole life, and it works for an
    /// in-memory wallet too. [`Self::unpinned`] skips the network check and
    /// the pin at sync time.
    ///
    /// # Errors
    ///
    /// - [`WalletError::NetworkUnknown`] when the wallet has no network, and
    ///   the node reports none.
    /// - [`WalletError::NetworkMismatch`] and [`WalletError::ChainMismatch`]
    ///   as above.
    /// - The [`WalletError`] of a replay that fails.
    pub async fn sync(self, source: &dyn SyncSource) -> Result<Wallet, WalletError> {
        Wallet::sync_inner(self.prepare(source).await?, None).await
    }

    /// Run the sync in a background task and stream progress events.
    ///
    /// Returns `(receiver, handle)`. The receiver emits [`SyncProgress`]
    /// events as each subscription replays. The [`SyncHandle`] resolves to
    /// the synced [`Wallet`] when all three subscriptions finish. The network
    /// check and the chain pin work run before anything spawns, which is why
    /// this is `async` and can refuse as [`Self::sync`] does.
    ///
    /// **Cancellation:** the spawned task lives exactly as long as both
    /// returned ends do. Dropping the progress receiver mid-sync cancels the
    /// task (the handle then resolves to [`WalletError::SyncCancelled`]),
    /// and dropping the [`SyncHandle`] aborts it. Either way, the three
    /// indexer WebSocket subscriptions are torn down promptly instead of
    /// running on with no consumer. Keep the receiver alive until you are
    /// done with the sync (the usual `while rx.recv().await` loop does this
    /// naturally: it only ends when the sync itself finishes). For a sync
    /// without progress events, use [`Self::sync`] instead.
    ///
    /// # Errors
    ///
    /// The errors of [`Self::sync`] that come before the replay, before
    /// anything spawns. After the spawn, the [`SyncHandle`] resolves to
    /// [`WalletError::SyncCancelled`] when the caller drops the receiver,
    /// to [`WalletError::SyncTaskJoin`] when the task panics or is aborted,
    /// and to the error of a replay that fails.
    pub async fn sync_with_progress(
        self,
        source: &dyn SyncSource,
    ) -> Result<(mpsc::Receiver<SyncProgress>, SyncHandle), WalletError> {
        let plan = self.prepare(source).await?;
        let (tx, rx) = mpsc::channel(64);
        let handle = tokio::spawn(async move {
            // A clone of the progress sender watches for receiver drop; the
            // original is consumed by the sync itself.
            let receiver_gone = tx.clone();
            let sync = Wallet::sync_inner(plan, Some(tx));
            tokio::select! {
                // Biased with the cancellation arm first: when the receiver
                // drop and a sync-side "receiver dropped" error become ready
                // in the same poll, the documented `SyncCancelled` must win.
                biased;
                // Receiver dropped mid-sync: the consumer abandoned the
                // stream. Dropping the sync future here tears down its
                // subscriptions and their WebSocket connections.
                _ = receiver_gone.closed() => Err(WalletError::SyncCancelled),
                result = sync => result,
            }
        });
        Ok((rx, SyncHandle { inner: handle }))
    }

    /// The chain work, done eagerly so what remains borrows nothing: settle
    /// the network, check it and the stored pin while the caller can still be
    /// refused, and take the fresh pin.
    async fn prepare(self, source: &dyn SyncSource) -> Result<SyncPlan, WalletError> {
        let WalletBuilder {
            seed,
            network,
            storage_dir,
            pin,
        } = self;
        let network = match network {
            Some(wallet) if pin => {
                if let Some(chain) = source.network().await
                    && chain != wallet
                {
                    return Err(WalletError::NetworkMismatch { wallet, chain });
                }
                wallet
            }
            Some(wallet) => wallet,
            None => source.network().await.ok_or(WalletError::NetworkUnknown)?,
        };
        let address = seed.unshielded_address(network.clone());

        let mut chain_pin = None;
        if pin {
            // Take the pin this sync carries before the check and the replay;
            // see `require_same_chain`.
            let candidate = current_pin(source).await;
            if let Some(dir) = storage_dir.as_deref()
                && let Some(stored) = Wallet::stored_chain_pin(dir, network.clone(), &address)?
            {
                let snapshot = Wallet::snapshot_path(dir, network.clone(), &address);
                require_same_chain(source, &stored, Some(&snapshot)).await?;
            }
            chain_pin = candidate;
        }

        Ok(SyncPlan {
            indexer_url: source.indexer_url().to_string(),
            seed,
            address,
            network,
            storage_dir,
            chain_pin,
        })
    }
}

/// Refuse a resume of state that `pin` ties to a chain the node no longer
/// runs.
///
/// A node that cannot answer leaves the cached state alone and logs a
/// warning. The error names `snapshot`, so the user knows what to remove, or
/// "the wallet snapshot" when there is no path to name.
///
/// The caller takes its replacement pin before this check, not after the
/// sync or resync commits. That pin is the mark the new state belongs to, so
/// a chain replaced at any point after it reads as replaced next time. Taken
/// afterwards, a swap during the sync would be stamped with the new chain's
/// own block, and every later check would pass against a chain that this
/// state never saw.
pub(crate) async fn require_same_chain(
    chain: &dyn ChainView,
    pin: &ChainPin,
    snapshot: Option<&Path>,
) -> Result<(), WalletError> {
    match verify_pin(chain, pin).await {
        ChainCheck::SameChain => Ok(()),
        ChainCheck::Unknown => {
            warn!(
                height = pin.height,
                "node could not answer for the pinned block; keeping the cached state"
            );
            Ok(())
        }
        ChainCheck::Replaced { found } => Err(WalletError::ChainMismatch {
            path: snapshot.map_or_else(
                || "the wallet snapshot".to_string(),
                |p| p.display().to_string(),
            ),
            pinned_height: pin.height,
            pinned_hash: pin.hash.clone(),
            found: found.map_or_else(|| "no block".to_string(), |h| format!("{h:#x}")),
        }),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use midnight_types::{BlockHash, LedgerVersion};

    use super::*;
    use crate::storage::{StoredMetadata, read_metadata, save_snapshot, wallet_storage_id};

    fn seed() -> Seed {
        Seed::from_bytes(&[0x11; 32]).unwrap()
    }

    fn stored_pin() -> ChainPin {
        ChainPin {
            height: 42,
            hash: format!("{:#x}", BlockHash::repeat_byte(0xaa)),
        }
    }

    /// Store a snapshot of `seed()` under `base` that carries `stored_pin()`.
    /// `prepare` reads only the metadata, so the snapshot has no state files.
    fn store_pinned_snapshot(base: &Path) {
        let address = seed().unshielded_address(Network::Undeployed);
        let metadata = StoredMetadata {
            generation: 0,
            ledger_version: LedgerVersion::V9,
            zswap_event_id: 0,
            dust_event_id: 0,
            last_block_height: 0,
            last_tx_id: None,
            chain_pin: Some(stored_pin()),
            unshielded_utxos: Vec::new(),
        };
        save_snapshot(
            base,
            Network::Undeployed.as_str(),
            &wallet_storage_id(&address),
            metadata,
            |_, _| Ok(()),
        )
        .unwrap();
    }

    /// A node whose chain holds the block `0xbbbb…` at every height. With
    /// `same_chain`, it is the snapshot's own chain: it holds `stored_pin()`
    /// at that pin's height. It reports `network` as the network it runs. It
    /// counts the queries it gets, the network query included.
    #[derive(Default)]
    struct Node {
        same_chain: bool,
        network: Option<Network>,
        queries: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl ChainView for Node {
        async fn block_hashes_at(&self, height: u64) -> Option<Vec<BlockHash>> {
            self.queries.fetch_add(1, Ordering::SeqCst);
            if self.same_chain && height == stored_pin().height {
                return Some(vec![BlockHash::repeat_byte(0xaa)]);
            }
            Some(vec![BlockHash::repeat_byte(0xbb)])
        }

        async fn finalized_height(&self) -> Option<u64> {
            self.queries.fetch_add(1, Ordering::SeqCst);
            Some(100)
        }
    }

    #[async_trait::async_trait]
    impl SyncSource for Node {
        fn indexer_url(&self) -> &str {
            // Never dialed: these tests stop at `prepare`.
            "http://127.0.0.1:1"
        }

        async fn network(&self) -> Option<Network> {
            self.queries.fetch_add(1, Ordering::SeqCst);
            self.network.clone()
        }
    }

    #[tokio::test]
    async fn a_default_sync_resumes_a_snapshot_on_its_own_chain_with_a_fresh_pin() {
        let base = tempfile::TempDir::new().unwrap();
        store_pinned_snapshot(base.path());
        let node = Node {
            same_chain: true,
            ..Node::default()
        };

        let result = WalletBuilder::new(seed())
            .with_network(Network::Undeployed)
            .with_storage(base.path())
            .prepare(&node)
            .await;
        let plan = match result {
            Ok(plan) => plan,
            Err(err) => panic!("a snapshot on its own chain must resume, got {err:?}"),
        };
        let fresh = ChainPin {
            height: 100,
            hash: format!("{:#x}", BlockHash::repeat_byte(0xbb)),
        };
        assert_eq!(plan.chain_pin, Some(fresh));
    }

    #[tokio::test]
    async fn a_default_sync_refuses_a_snapshot_from_a_replaced_chain() {
        let base = tempfile::TempDir::new().unwrap();
        store_pinned_snapshot(base.path());
        let node = Node {
            same_chain: false,
            ..Node::default()
        };

        let result = WalletBuilder::new(seed())
            .with_network(Network::Undeployed)
            .with_storage(base.path())
            .prepare(&node)
            .await;
        let path = match result {
            Err(WalletError::ChainMismatch { path, .. }) => path,
            Err(other) => panic!("expected ChainMismatch, got {other:?}"),
            Ok(_) => panic!("a default sync must refuse a snapshot from a replaced chain"),
        };
        // The path is the recovery instruction, so it must name the snapshot.
        let refused = read_metadata(Path::new(&path))
            .unwrap()
            .and_then(|m| m.chain_pin);
        assert_eq!(refused, Some(stored_pin()), "{path} holds no such snapshot");
    }

    /// The stored pin is from a replaced chain, and the node runs another
    /// network than the one the wallet names, so a sync that still checks
    /// either one refuses.
    #[tokio::test]
    async fn an_unpinned_sync_with_a_network_asks_the_node_nothing() {
        let base = tempfile::TempDir::new().unwrap();
        store_pinned_snapshot(base.path());
        let node = Node {
            same_chain: false,
            network: Some(Network::Preprod),
            ..Node::default()
        };

        let result = WalletBuilder::new(seed())
            .with_network(Network::Undeployed)
            .with_storage(base.path())
            .unpinned()
            .prepare(&node)
            .await;
        let plan = match result {
            Ok(plan) => plan,
            Err(err) => panic!("an unpinned sync with a network refuses nothing, got {err:?}"),
        };
        assert_eq!(node.queries.load(Ordering::SeqCst), 0);
        assert_eq!(plan.chain_pin, None);
    }

    /// `prepare` runs before the replay, so a refusal here costs no indexer
    /// work.
    #[tokio::test]
    async fn the_network_check_runs_in_prepare() {
        // The network the node reports, and the network a refusal names.
        let cases = [
            (Some(Network::Preprod), Some(Network::Preprod)),
            // A node with no network id set reports none. Refusing it would
            // refuse every sync against that node.
            (None, None),
            (Some(Network::Undeployed), None),
        ];
        for (reported, refused_as) in cases {
            let node = Node {
                network: reported.clone(),
                ..Node::default()
            };
            let result = WalletBuilder::new(seed())
                .with_network(Network::Undeployed)
                .prepare(&node)
                .await;
            match (result, refused_as) {
                (Err(WalletError::NetworkMismatch { wallet, chain }), Some(expected)) => {
                    assert_eq!(wallet, Network::Undeployed);
                    assert_eq!(chain, expected);
                    // Before the pin: on a stored snapshot, a `ChainMismatch`
                    // would tell the user to remove a valid cache.
                    assert_eq!(
                        node.queries.load(Ordering::SeqCst),
                        1,
                        "a refused sync sends the node only the network query"
                    );
                }
                (Ok(_), None) => {}
                (Err(err), _) => panic!("node reports {reported:?}: unexpected {err:?}"),
                (Ok(_), Some(_)) => panic!("node reports {reported:?}: the sync must refuse"),
            }
        }
    }

    /// A wallet with no network takes the node's, pinned or not, and its
    /// address follows that network. Unpinned, the sync asks the node only
    /// that query and takes no pin. A node that reports none leaves no
    /// network to take.
    #[tokio::test]
    async fn a_wallet_with_no_network_takes_the_nodes() {
        for pin in [true, false] {
            let builder = || {
                let builder = WalletBuilder::new(seed());
                if pin { builder } else { builder.unpinned() }
            };
            let node = Node {
                network: Some(Network::Preprod),
                ..Node::default()
            };
            let plan = match builder().prepare(&node).await {
                Ok(plan) => plan,
                Err(err) => panic!("pin {pin}: the node names a network, got {err:?}"),
            };
            assert_eq!(plan.network, Network::Preprod, "pin {pin}");
            assert_eq!(
                plan.address,
                seed().unshielded_address(Network::Preprod),
                "pin {pin}"
            );
            assert_eq!(plan.chain_pin.is_some(), pin, "pin {pin}");
            let queries = if pin { 3 } else { 1 };
            assert_eq!(node.queries.load(Ordering::SeqCst), queries, "pin {pin}");

            let silent = Node::default();
            match builder().prepare(&silent).await {
                Err(WalletError::NetworkUnknown) => {}
                Err(err) => panic!("pin {pin}: expected NetworkUnknown, got {err:?}"),
                Ok(plan) => panic!("pin {pin}: the sync took {} from nowhere", plan.network),
            }
        }
    }

    #[tokio::test]
    async fn sync_handle_maps_join_error_to_wallet_error() {
        let handle: JoinHandle<Result<Wallet, WalletError>> = tokio::spawn(async {
            std::future::pending::<()>().await;
            unreachable!()
        });
        handle.abort();
        let sync = SyncHandle { inner: handle };
        let Err(err) = sync.await else {
            panic!("aborted task should surface as a WalletError");
        };
        assert!(
            matches!(err, WalletError::SyncTaskJoin(_)),
            "expected SyncTaskJoin, got {err:?}"
        );
    }

    #[tokio::test]
    async fn sync_handle_passes_through_inner_error() {
        let handle: JoinHandle<Result<Wallet, WalletError>> =
            tokio::spawn(async { Err(WalletError::SyncCancelled) });
        let sync = SyncHandle { inner: handle };
        let Err(err) = sync.await else {
            panic!("inner Err should propagate");
        };
        assert!(
            matches!(err, WalletError::SyncCancelled),
            "expected SyncCancelled, got {err:?}"
        );
    }
}
