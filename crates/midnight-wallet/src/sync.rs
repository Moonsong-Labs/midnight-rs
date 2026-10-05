//! Build a synced [`Wallet`] from a [`SyncSource`], as a standalone step.
//!
//! The wallet is constructed on its own and attached afterwards. The source
//! is a trait object, so nothing here names a provider:
//!
//! ```rust,no_run
//! # use midnight_provider::MidnightProvider;
//! # use midnight_wallet::{LocalWallet, Network, Wallet, WalletSeed};
//! # const NODE_URL: &str = "ws://localhost:9944";
//! # const INDEXER_URL: &str = "http://localhost:8088";
//! # async fn example(seed: WalletSeed) -> Result<(), Box<dyn std::error::Error>> {
//! let provider = MidnightProvider::new(NODE_URL, INDEXER_URL)?;
//! let wallet = Wallet::sync(&provider, seed, Network::Undeployed).await?;
//! let provider = provider.with_wallet(LocalWallet::new(wallet));
//! # Ok(())
//! # }
//! ```
//!
//! Every sync pins the wallet to the chain by default, as [`Wallet::sync`]
//! describes.

use std::path::PathBuf;

use midnight_types::chain_pin::{ChainCheck, ChainPin, SyncSource, current_pin, verify_pin};
use midnight_types::{Network, WalletError, WalletSeed};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::warn;

use crate::Wallet;

/// Progress updates emitted during wallet sync.
#[derive(Debug, Clone)]
pub enum SyncProgress {
    Resuming {
        zswap_event_id: i64,
        dust_event_id: i64,
    },
    ZswapEvents {
        current: i64,
        max: i64,
    },
    ZswapComplete {
        events: u64,
    },
    DustEvents {
        current: i64,
        max: i64,
    },
    DustComplete {
        events: u64,
    },
    UnshieldedCaughtUp {
        utxos: usize,
    },
}

impl Wallet {
    /// Sync a wallet from `source`, from its genesis or from a stored
    /// snapshot.
    ///
    /// The sync replays the source's indexer, and the wallet keeps that
    /// indexer for its later resyncs. `MidnightProvider` is a source.
    ///
    /// The source's node pins the wallet to the chain. This pin is the
    /// chain-reset guard. A snapshot's cursors are counts, so a snapshot
    /// from a replaced chain resumes cleanly and reports the dead chain's
    /// balance. To catch this, the sync checks a stored snapshot's pin
    /// against the node before the replay starts. A pin that the chain no
    /// longer holds fails the sync with [`WalletError::ChainMismatch`].
    ///
    /// A node that cannot answer does not fail the sync, because a pruned
    /// archive must not condemn a healthy wallet. The synced wallet also
    /// carries a fresh pin, which every resync checks again. Thus the guard
    /// holds for the wallet's whole life, and it works for an in-memory
    /// wallet too. [`WalletSyncBuilder::unpinned`] skips the pin at sync
    /// time.
    ///
    /// Returns a [`WalletSyncBuilder`] that defers the actual work. Configure
    /// optional persistence with [`WalletSyncBuilder::with_storage`], then
    /// either `.await` for the one-shot path or `.stream()` for streamed
    /// progress events. The two paths share their entire body; they only
    /// differ in whether a progress sender is attached and whether the sync
    /// runs in the current task or a spawned one.
    pub fn sync(
        source: &dyn SyncSource,
        seed: impl Into<WalletSeed>,
        network: impl Into<Network>,
    ) -> WalletSyncBuilder<'_> {
        WalletSyncBuilder {
            source,
            seed: seed.into(),
            network: network.into(),
            storage_dir: None,
            pin: true,
        }
    }
}

/// Handle to the background task spawned by
/// [`WalletSyncBuilder::stream`].
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
/// `SyncHandle`, spawn the one-shot path yourself. The builder borrows its
/// source, so move an `Arc` of the source into the task:
/// `tokio::spawn(async move { Wallet::sync(&*source, seed, network).await })`.
pub struct SyncHandle {
    inner: JoinHandle<Result<Wallet, WalletError>>,
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

/// Builder returned by [`Wallet::sync`].
///
/// Holds the configuration (source, seed, network, optional storage dir, and
/// whether to pin the chain) until the caller selects a sync path:
///
/// - `.await` — runs the sync in the current task, returns the synced
///   [`Wallet`]. No progress events.
/// - [`stream()`](Self::stream) — spawns the sync in a background task and
///   returns `(receiver, handle)`. The receiver emits [`SyncProgress`] events;
///   the [`SyncHandle`] resolves to the synced wallet when sync completes.
#[must_use = "the sync does nothing until awaited or streamed"]
pub struct WalletSyncBuilder<'a> {
    source: &'a dyn SyncSource,
    seed: WalletSeed,
    network: Network,
    storage_dir: Option<PathBuf>,
    pin: bool,
}

/// The owned inputs a sync runs on, once the chain work is done. Holding no
/// borrow is what lets [`WalletSyncBuilder::stream`] spawn.
struct SyncPlan {
    indexer_url: String,
    seed: WalletSeed,
    address: String,
    network: Network,
    storage_dir: Option<PathBuf>,
    chain_pin: Option<ChainPin>,
}

impl<'a> WalletSyncBuilder<'a> {
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

    /// Skip the pin at sync time: the sync asks the node nothing.
    ///
    /// The sync still keeps a stored snapshot's pin, and every later resync
    /// checks that pin. A wallet synced unpinned with no stored pin stays
    /// unpinned for its life. [`Wallet::sync`] describes the chain-reset
    /// guard.
    pub fn unpinned(mut self) -> Self {
        self.pin = false;
        self
    }

    /// The chain work, done eagerly so what remains borrows nothing: check
    /// the stored pin while the caller can still be refused, and take the
    /// fresh one.
    async fn prepare(self) -> Result<SyncPlan, WalletError> {
        let WalletSyncBuilder {
            source,
            seed,
            network,
            storage_dir,
            pin,
        } = self;
        let address = midnight_types::address::derive_unshielded(&seed, network.clone());

        let mut chain_pin = None;
        if pin {
            // Take the pin this sync will carry before checking the stored
            // one, and before the replay it precedes. A chain replaced at any
            // point after this reads as replaced next time. Taken afterwards,
            // a swap during the sync would be stamped with the new chain's own
            // block, and the state resumed from the old one would never be
            // caught.
            let candidate = current_pin(source).await;
            if let Some(dir) = storage_dir.as_deref()
                && let Some(stored) = Wallet::stored_chain_pin(dir, network.clone(), &address)?
            {
                match verify_pin(source, &stored).await {
                    ChainCheck::SameChain => {}
                    ChainCheck::Unknown => {
                        warn!(
                            height = stored.height,
                            "node could not answer for the pinned block; keeping the cached state"
                        );
                    }
                    ChainCheck::Replaced { found } => {
                        return Err(WalletError::ChainMismatch {
                            path: Wallet::snapshot_path(dir, network.clone(), &address)
                                .display()
                                .to_string(),
                            pinned_height: stored.height,
                            pinned_hash: stored.hash,
                            found: found.unwrap_or_else(|| "no block".to_string()),
                        });
                    }
                }
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

    /// Run the sync in a background task and stream progress events.
    ///
    /// Returns `(receiver, handle)`. The receiver emits [`SyncProgress`]
    /// events as each subscription replays. The [`SyncHandle`] resolves to
    /// the synced [`Wallet`] when all three subscriptions finish. The chain
    /// pin work runs before anything spawns, which is why this is `async`
    /// and can refuse with [`WalletError::ChainMismatch`]. An
    /// [`unpinned`](Self::unpinned) sync has no chain pin work.
    ///
    /// **Cancellation:** the spawned task lives exactly as long as both
    /// returned ends do. Dropping the progress receiver mid-sync cancels the
    /// task (the handle then resolves to [`WalletError::SyncCancelled`]),
    /// and dropping the [`SyncHandle`] aborts it — either way the three
    /// indexer WebSocket subscriptions are torn down promptly instead of
    /// running on with no consumer. Keep the receiver alive until you are
    /// done with the sync (the usual `while rx.recv().await` loop does this
    /// naturally: it only ends when the sync itself finishes). For a sync
    /// without progress events, use the plain `.await` path instead of
    /// `stream()`.
    pub async fn stream(self) -> Result<(mpsc::Receiver<SyncProgress>, SyncHandle), WalletError> {
        let plan = self.prepare().await?;
        let (tx, rx) = mpsc::channel(64);
        let handle = tokio::spawn(async move {
            // A clone of the progress sender watches for receiver drop; the
            // original is consumed by the sync itself.
            let receiver_gone = tx.clone();
            let sync = Wallet::sync_inner(
                &plan.indexer_url,
                plan.seed,
                &plan.address,
                plan.network,
                plan.storage_dir.as_deref(),
                plan.chain_pin,
                Some(tx),
            );
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
}

impl<'a> std::future::IntoFuture for WalletSyncBuilder<'a> {
    type Output = Result<Wallet, WalletError>;
    type IntoFuture =
        std::pin::Pin<Box<dyn std::future::Future<Output = Self::Output> + Send + 'a>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let plan = self.prepare().await?;
            Wallet::sync_inner(
                &plan.indexer_url,
                plan.seed,
                &plan.address,
                plan.network,
                plan.storage_dir.as_deref(),
                plan.chain_pin,
                None,
            )
            .await
        })
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use midnight_types::LedgerVersion;
    use midnight_types::chain_pin::ChainView;

    use super::*;
    use crate::storage::{StoredMetadata, read_metadata, save_snapshot, wallet_storage_id};

    fn seed() -> WalletSeed {
        WalletSeed::try_from_hex_str(&"11".repeat(32)).unwrap()
    }

    fn stored_pin() -> ChainPin {
        ChainPin {
            height: 42,
            hash: "0xaa".to_string(),
        }
    }

    /// Store a snapshot of `seed()` under `base` that carries `stored_pin()`.
    /// `prepare` reads only the metadata, so the snapshot has no state files.
    fn store_pinned_snapshot(base: &Path) {
        let address = midnight_types::address::derive_unshielded(&seed(), Network::Undeployed);
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

    /// A node whose chain holds the block `0xbb` at every height. With
    /// `same_chain`, it is the snapshot's own chain: it holds `stored_pin()`
    /// at that pin's height. It counts the questions it gets.
    #[derive(Default)]
    struct Node {
        same_chain: bool,
        questions: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl ChainView for Node {
        async fn block_hashes_at(&self, height: u64) -> Option<Vec<String>> {
            self.questions.fetch_add(1, Ordering::SeqCst);
            let stored = stored_pin();
            if self.same_chain && height == stored.height {
                return Some(vec![stored.hash]);
            }
            Some(vec!["0xbb".to_string()])
        }

        async fn finalized_height(&self) -> Option<u64> {
            self.questions.fetch_add(1, Ordering::SeqCst);
            Some(100)
        }
    }

    impl SyncSource for Node {
        fn indexer_url(&self) -> &str {
            // Never dialed: these tests stop at `prepare`.
            "http://127.0.0.1:1"
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

        let result = Wallet::sync(&node, seed(), Network::Undeployed)
            .with_storage(base.path())
            .prepare()
            .await;
        let plan = match result {
            Ok(plan) => plan,
            Err(err) => panic!("a snapshot on its own chain must resume, got {err:?}"),
        };
        let fresh = ChainPin {
            height: 100,
            hash: "0xbb".to_string(),
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

        let result = Wallet::sync(&node, seed(), Network::Undeployed)
            .with_storage(base.path())
            .prepare()
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

    /// The stored pin is from a replaced chain, so a sync that still checks
    /// it refuses.
    #[tokio::test]
    async fn an_unpinned_sync_asks_the_node_nothing() {
        let base = tempfile::TempDir::new().unwrap();
        store_pinned_snapshot(base.path());
        let node = Node {
            same_chain: false,
            ..Node::default()
        };

        let result = Wallet::sync(&node, seed(), Network::Undeployed)
            .with_storage(base.path())
            .unpinned()
            .prepare()
            .await;
        let plan = match result {
            Ok(plan) => plan,
            Err(err) => panic!("an unpinned sync refuses nothing, got {err:?}"),
        };
        assert_eq!(node.questions.load(Ordering::SeqCst), 0);
        assert_eq!(plan.chain_pin, None);
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
