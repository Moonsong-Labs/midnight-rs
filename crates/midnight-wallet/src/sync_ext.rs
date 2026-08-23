//! [`SyncWalletExt`]: sync a [`LocalWallet`] from a provider's indexer and
//! attach it, without the provider crate naming this implementation.

use midnight_provider::{MidnightProvider, ProviderError};
use midnight_types::Network;
use midnight_types::WalletSeed;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::local::LocalWallet;
use crate::state::{SyncProgress, Wallet};

/// Sync-and-attach for [`MidnightProvider`]: build the [`LocalWallet`] this
/// crate implements and hand it to the provider.
///
/// An extension trait, so the provider crate stays free of any wallet
/// implementation. Bring it into scope to call
/// `provider.sync_wallet(seed, network)`.
pub trait SyncWalletExt {
    /// Sync a wallet from the indexer and attach it to this provider.
    ///
    /// Convenience builder around the wallet's sync logic that uses this
    /// provider's indexer URL — callers don't repeat URLs that already live on
    /// the provider:
    ///
    /// ```rust,ignore
    /// // Simple one-shot sync.
    /// let provider = MidnightProvider::new(NODE_URL, INDEXER_URL)?
    ///     .sync_wallet(seed, Network::Undeployed)
    ///     .await?;
    ///
    /// // With persistence + streamed progress.
    /// let (mut rx, handle) = MidnightProvider::new(NODE_URL, INDEXER_URL)?
    ///     .sync_wallet(seed, Network::Preprod)
    ///     .with_storage(storage_dir)
    ///     .stream();
    /// while let Some(p) = rx.recv().await { /* render */ }
    /// let provider = handle.await?;
    /// ```
    ///
    /// Returns a [`SyncWalletBuilder`] that defers the actual work. Configure
    /// optional persistence with [`SyncWalletBuilder::with_storage`], then
    /// either `.await` for the one-shot path or `.stream()` for streamed
    /// progress events. The two paths share their entire body — they only
    /// differ in whether a progress sender is attached and whether the sync
    /// runs in the current task or a spawned one.
    ///
    /// If a wallet is already attached (via [`MidnightProvider::with_wallet`] or a previous
    /// `sync_wallet` call), it is replaced by the newly synced wallet.
    ///
    /// To incrementally refresh an already-attached wallet without a full
    /// resync, use [`MidnightProvider::resync_wallet`].
    fn sync_wallet(
        self,
        seed: impl Into<WalletSeed>,
        network: impl Into<Network>,
    ) -> SyncWalletBuilder;
}

impl SyncWalletExt for MidnightProvider {
    fn sync_wallet(
        self,
        seed: impl Into<WalletSeed>,
        network: impl Into<Network>,
    ) -> SyncWalletBuilder {
        SyncWalletBuilder {
            provider: self,
            seed: seed.into(),
            network: network.into(),
            storage_dir: None,
        }
    }
}

/// Handle to the background task spawned by
/// [`SyncWalletBuilder::stream`].
///
/// Awaiting it yields the synced [`MidnightProvider`] — a single `?` is
/// enough. A panic or cancellation of the spawned task surfaces as
/// [`ProviderError::SyncTaskJoin`]; the inner sync error path surfaces as
/// the matching `ProviderError` variant.
///
/// **Dropping the handle cancels the sync.** The handle is the only way to
/// obtain the synced provider, so once it is dropped the sync's result is
/// unobservable and letting it run would only keep three indexer WebSocket
/// subscriptions alive for nothing. To run a sync without holding a
/// `SyncHandle`, spawn the one-shot path yourself:
/// `tokio::spawn(provider.sync_wallet(seed, network).into_future())`.
pub struct SyncHandle {
    inner: JoinHandle<Result<MidnightProvider, ProviderError>>,
}

impl SyncHandle {
    pub(crate) fn from_handle(inner: JoinHandle<Result<MidnightProvider, ProviderError>>) -> Self {
        Self { inner }
    }
}

impl Drop for SyncHandle {
    fn drop(&mut self) {
        // Cancel-on-drop (see the struct docs). Aborting the task drops its
        // in-flight `Subscription` handles, which tear down their WebSocket
        // reader tasks — no orphaned subscriptions survive the handle. The
        // sync task holds no locks at any await point (the wallet reaches the
        // provider only after the sync completes), so an abort cannot strand
        // a lock. No-op if the task already finished, e.g.
        // after the handle was awaited to completion.
        self.inner.abort();
    }
}

impl std::future::Future for SyncHandle {
    type Output = Result<MidnightProvider, ProviderError>;

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        std::pin::Pin::new(&mut self.inner)
            .poll(cx)
            .map(|outer| match outer {
                Ok(inner) => inner,
                Err(join_err) => Err(join_err.into()),
            })
    }
}

/// Builder returned by [`MidnightProvider::sync_wallet`].
///
/// Holds the configuration (seed, network, optional storage dir) until the
/// caller selects a sync path:
///
/// - `.await` — runs the sync in the current task, returns the synced
///   [`MidnightProvider`]. No progress events.
/// - [`stream()`](Self::stream) — spawns the sync in a background task and
///   returns `(receiver, handle)`. The receiver emits [`SyncProgress`] events;
///   the [`SyncHandle`] resolves to the synced provider when sync completes.
pub struct SyncWalletBuilder {
    provider: MidnightProvider,
    seed: WalletSeed,
    network: Network,
    storage_dir: Option<std::path::PathBuf>,
}

impl SyncWalletBuilder {
    /// Persist sync progress + recovered state under `dir`. Without this call,
    /// the wallet runs in-memory only. The directory is retained: every
    /// successful resync re-saves the wallet and each transfer build
    /// persists its pending reservation.
    ///
    /// See [`docs/wallet.md`](https://github.com/RomarQ/midnight-rs/blob/main/docs/wallet.md#persistence)
    /// for the on-disk layout.
    pub fn with_storage(mut self, dir: impl Into<std::path::PathBuf>) -> Self {
        self.storage_dir = Some(dir.into());
        self
    }

    /// Run the sync in a background task and stream progress events.
    ///
    /// Returns `(receiver, handle)`. The receiver emits [`SyncProgress`]
    /// events as each subscription replays. The [`SyncHandle`] resolves to
    /// the synced [`MidnightProvider`] when all three subscriptions finish.
    ///
    /// **Cancellation:** the spawned task lives exactly as long as both
    /// returned ends do. Dropping the progress receiver mid-sync cancels the
    /// task (the handle then resolves to [`ProviderError::SyncCancelled`]),
    /// and dropping the [`SyncHandle`] aborts it — either way the three
    /// indexer WebSocket subscriptions are torn down promptly instead of
    /// running on with no consumer. Keep the receiver alive until you are
    /// done with the sync (the usual `while rx.recv().await` loop does this
    /// naturally: it only ends when the sync itself finishes). For a sync
    /// without progress events, use the plain `.await` path instead of
    /// `stream()`.
    pub fn stream(self) -> (mpsc::Receiver<SyncProgress>, SyncHandle) {
        let (tx, rx) = mpsc::channel(64);
        let SyncWalletBuilder {
            provider,
            seed,
            network,
            storage_dir,
        } = self;
        let indexer_url = provider.indexer_url().to_string();
        let handle = tokio::spawn(async move {
            let address = midnight_types::address::derive_unshielded(&seed, network.clone());
            let chain_pin =
                verify_chain_and_pin(&provider, storage_dir.as_deref(), &network, &address).await?;
            // A clone of the progress sender watches for receiver drop; the
            // original is consumed by the sync itself.
            let receiver_gone = tx.clone();
            let sync = Wallet::sync_inner(
                &indexer_url,
                seed,
                &address,
                network,
                storage_dir.as_deref(),
                chain_pin,
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
                _ = receiver_gone.closed() => Err(ProviderError::SyncCancelled),
                result = sync => {
                    Ok(provider.with_wallet(LocalWallet::new(result?)))
                }
            }
        });
        (rx, SyncHandle::from_handle(handle))
    }
}

impl std::future::IntoFuture for SyncWalletBuilder {
    type Output = Result<MidnightProvider, ProviderError>;
    type IntoFuture =
        std::pin::Pin<Box<dyn std::future::Future<Output = Self::Output> + Send + 'static>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let SyncWalletBuilder {
                provider,
                seed,
                network,
                storage_dir,
            } = self;
            let address = midnight_types::address::derive_unshielded(&seed, network.clone());
            let chain_pin =
                verify_chain_and_pin(&provider, storage_dir.as_deref(), &network, &address).await?;
            let wallet = Wallet::sync_inner(
                provider.indexer_url(),
                seed,
                &address,
                network,
                storage_dir.as_deref(),
                chain_pin,
                None,
            )
            .await?;
            Ok(provider.with_wallet(LocalWallet::new(wallet)))
        })
    }
}

/// Check a snapshot against the chain before a resume trusts it, and
/// return the pin the sync should persist.
///
/// The wallet keeps event cursors, which are counts, so a snapshot taken
/// from a chain that has since been replaced resumes without complaint
/// and reports the dead chain's balance. Only the node can settle which
/// chain this is, which is why the check asks the provider.
///
/// A node that cannot answer leaves the snapshot alone: a pruned archive
/// must not condemn a healthy wallet.
/// Check a snapshot against the chain before a resume trusts it, and
/// return the pin the sync should persist.
///
/// The wallet keeps event cursors, which are counts, so a snapshot taken
/// from a chain that has since been replaced resumes without complaint
/// and reports the dead chain's balance. Only the node can settle which
/// chain this is, which is why the check lives here and not in the wallet.
///
/// A node that cannot answer leaves the snapshot alone: a pruned archive
/// must not condemn a healthy wallet.
async fn verify_chain_and_pin(
    provider: &MidnightProvider,
    storage_dir: Option<&std::path::Path>,
    network: &Network,
    address: &str,
) -> Result<Option<midnight_types::chain_pin::ChainPin>, ProviderError> {
    let Some(dir) = storage_dir else {
        return Ok(None);
    };

    if let Some(pin) = Wallet::stored_chain_pin(dir, network.clone(), address)? {
        let path = Wallet::snapshot_path(dir, network.clone(), address)
            .display()
            .to_string();
        provider.check_chain_pin_at(&pin, path).await?;
    }

    Ok(provider.current_chain_pin().await)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn sync_handle_maps_join_error_to_provider_error() {
        let handle: JoinHandle<Result<MidnightProvider, ProviderError>> = tokio::spawn(async {
            std::future::pending::<()>().await;
            unreachable!()
        });
        handle.abort();
        let sync = SyncHandle::from_handle(handle);
        let Err(err) = sync.await else {
            panic!("aborted task should surface as a ProviderError");
        };
        assert!(
            matches!(err, ProviderError::SyncTaskJoin(_)),
            "expected SyncTaskJoin, got {err:?}"
        );
    }

    #[tokio::test]
    async fn sync_handle_passes_through_inner_error() {
        let handle: JoinHandle<Result<MidnightProvider, ProviderError>> =
            tokio::spawn(async { Err(ProviderError::NoWallet) });
        let sync = SyncHandle::from_handle(handle);
        let Err(err) = sync.await else {
            panic!("inner Err should propagate");
        };
        assert!(
            matches!(err, ProviderError::NoWallet),
            "expected NoWallet, got {err:?}"
        );
    }
}
