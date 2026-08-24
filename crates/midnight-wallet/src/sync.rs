//! Build a synced [`Wallet`] from an indexer, as a standalone step.
//!
//! The wallet is constructed on its own and attached afterwards, so nothing
//! here names a provider:
//!
//! ```rust,ignore
//! let provider = MidnightProvider::new(NODE_URL, INDEXER_URL)?;
//! let wallet = Wallet::sync(provider.indexer_url(), seed, Network::Undeployed)
//!     .pinned_to(&provider)
//!     .await?;
//! let provider = provider.with_wallet(LocalWallet::new(wallet));
//! ```
//!
//! [`WalletSyncBuilder::pinned_to`] is the chain-reset guard. It takes any
//! [`ChainView`] (the provider implements it over its node RPCs), checks a
//! stored snapshot's pin before the replay starts, and gives the wallet a
//! fresh pin to carry. Without it the wallet is unpinned.

use std::path::PathBuf;

use midnight_types::chain_pin::{ChainCheck, ChainPin, ChainView, current_pin, verify_pin};
use midnight_types::{Network, WalletError, WalletSeed};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::warn;

use crate::state::{SyncProgress, Wallet};

impl Wallet {
    /// Sync a wallet against an indexer, from its genesis or from a stored
    /// snapshot.
    ///
    /// Returns a [`WalletSyncBuilder`] that defers the actual work. Configure
    /// optional persistence with [`WalletSyncBuilder::with_storage`] and the
    /// chain-reset guard with [`WalletSyncBuilder::pinned_to`], then either
    /// `.await` for the one-shot path or `.stream()` for streamed progress
    /// events. The two paths share their entire body; they only differ in
    /// whether a progress sender is attached and whether the sync runs in the
    /// current task or a spawned one.
    pub fn sync<'a>(
        indexer_url: impl Into<String>,
        seed: impl Into<WalletSeed>,
        network: impl Into<Network>,
    ) -> WalletSyncBuilder<'a> {
        WalletSyncBuilder {
            indexer_url: indexer_url.into(),
            seed: seed.into(),
            network: network.into(),
            storage_dir: None,
            chain: None,
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
/// `SyncHandle`, spawn the one-shot path yourself:
/// `tokio::spawn(Wallet::sync(url, seed, network).into_future())`.
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
/// Holds the configuration (indexer URL, seed, network, optional storage dir
/// and chain view) until the caller selects a sync path:
///
/// - `.await` — runs the sync in the current task, returns the synced
///   [`Wallet`]. No progress events.
/// - [`stream()`](Self::stream) — spawns the sync in a background task and
///   returns `(receiver, handle)`. The receiver emits [`SyncProgress`] events;
///   the [`SyncHandle`] resolves to the synced wallet when sync completes.
pub struct WalletSyncBuilder<'a> {
    indexer_url: String,
    seed: WalletSeed,
    network: Network,
    storage_dir: Option<PathBuf>,
    chain: Option<&'a dyn ChainView>,
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

    /// Guard this wallet against a chain reset, with `chain` answering the
    /// two questions a pin asks a node.
    ///
    /// Before the replay starts, a stored snapshot's pin is checked against
    /// the chain: a snapshot's cursors are counts, so one taken from a chain
    /// that has since been replaced resumes without complaint and reports the
    /// dead chain's balance. A pin the chain no longer holds fails the sync
    /// with [`WalletError::ChainMismatch`]; a node that cannot answer changes
    /// nothing, because a pruned archive must not condemn a healthy wallet.
    ///
    /// The synced wallet also carries a fresh pin, which every resync checks
    /// again, so the guard holds for the wallet's whole life and works for an
    /// in-memory wallet too. Without this call the wallet is unpinned.
    ///
    /// `MidnightProvider` implements [`ChainView`] over its node RPCs.
    pub fn pinned_to(mut self, chain: &'a dyn ChainView) -> Self {
        self.chain = Some(chain);
        self
    }

    /// The chain work, done eagerly so what remains borrows nothing: check
    /// the stored pin while the caller can still be refused, and take the
    /// fresh one.
    async fn prepare(self) -> Result<SyncPlan, WalletError> {
        let WalletSyncBuilder {
            indexer_url,
            seed,
            network,
            storage_dir,
            chain,
        } = self;
        let address = midnight_types::address::derive_unshielded(&seed, network.clone());

        let mut chain_pin = None;
        if let Some(chain) = chain {
            if let Some(dir) = storage_dir.as_deref() {
                if let Some(pin) = Wallet::stored_chain_pin(dir, network.clone(), &address)? {
                    match verify_pin(chain, &pin).await {
                        ChainCheck::SameChain => {}
                        ChainCheck::Unknown => {
                            warn!(
                                height = pin.height,
                                "node could not answer for the pinned block; keeping the cached state"
                            );
                        }
                        ChainCheck::Replaced { found } => {
                            return Err(WalletError::ChainMismatch {
                                path: Wallet::snapshot_path(dir, network.clone(), &address)
                                    .display()
                                    .to_string(),
                                pinned_height: pin.height,
                                pinned_hash: pin.hash.clone(),
                                found: found.unwrap_or_else(|| "no block".to_string()),
                            });
                        }
                    }
                }
            }
            chain_pin = current_pin(chain).await;
        }

        Ok(SyncPlan {
            indexer_url,
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
    /// work from [`Self::pinned_to`] runs before anything spawns, which is
    /// why this is `async` and can refuse with
    /// [`WalletError::ChainMismatch`].
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
    use super::*;

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
