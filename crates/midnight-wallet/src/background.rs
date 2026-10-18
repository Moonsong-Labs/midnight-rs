//! [`BackgroundSync`]: the handle of the task that resyncs a [`LocalWallet`]
//! until the handle drops.

use std::sync::Arc;
use std::time::Duration;

use midnight_types::WalletError;
use midnight_types::chain_pin::ChainView;
use midnight_wallet_facade::WalletFacade;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use crate::{LocalWallet, Wallet};

/// The background sync of a [`LocalWallet`], from
/// [`LocalWallet::sync_in_background`].
///
/// Dropping it stops the sync at the task's next await. A pass in flight
/// leaves the wallet at the state before the pass or at its whole commit.
#[must_use = "dropping a BackgroundSync stops the sync"]
pub struct BackgroundSync {
    task: JoinHandle<()>,
    status: watch::Receiver<Option<Arc<WalletError>>>,
    wallet: Arc<LocalWallet>,
}

impl BackgroundSync {
    /// The error of the last pass, or `None` when that pass committed.
    ///
    /// `None` also means that the sync still runs. When the task has ended,
    /// it returns [`WalletError::SyncTaskJoin`]. While the handle lives, only
    /// a pass that panics or a runtime that shuts down ends the task. Before
    /// the first pass ends, it returns `None`. A later pass that commits
    /// clears the error.
    pub fn last_error(&self) -> Option<Arc<WalletError>> {
        if self.task.is_finished() {
            return Some(Arc::new(stopped()));
        }
        self.status.borrow().clone()
    }

    /// Wait until `done` holds for the state of this wallet.
    ///
    /// `done` runs on the state at the call. It runs again after each commit
    /// of a resync or a shielded rescan. That commit can come from this sync,
    /// from the resync of a build through the provider, or from a direct call.
    /// It runs under the read lock of the wallet, so keep it short and do not
    /// block in it.
    ///
    /// A failed pass does not end the wait, because a later pass can commit.
    /// The wait has no deadline, so wrap it in `tokio::time::timeout`. After a
    /// timeout, [`Self::last_error`] tells whether the passes fail.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use std::time::Duration;
    ///
    /// use midnight_provider::MidnightProvider;
    /// use midnight_wallet::{BackgroundSync, SpentInputs};
    ///
    /// # async fn example(
    /// #     provider: &MidnightProvider,
    /// #     sync: &BackgroundSync,
    /// #     spent: &SpentInputs,
    /// # ) -> Result<(), Box<dyn std::error::Error>> {
    /// // `spent` holds the spends of a transaction that applied.
    /// tokio::time::timeout(Duration::from_secs(60), sync.wait_until(|w| w.has_observed(spent)))
    ///     .await??;
    ///
    /// // Wait for a new shielded coin.
    /// let count = provider.balance().await?.shielded.total_count;
    /// tokio::time::timeout(
    ///     Duration::from_secs(60),
    ///     sync.wait_until(|w| w.balance().shielded.total_count > count),
    /// )
    /// .await??;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// [`WalletError::SyncTaskJoin`] when the background task has ended. While
    /// the handle lives, only a pass that panics or a runtime that shuts down
    /// ends the task.
    ///
    /// # Cancellation
    ///
    /// Dropping the future is safe, because it holds no lock across an await.
    pub async fn wait_until<F>(&self, mut done: F) -> Result<(), WalletError>
    where
        F: FnMut(&Wallet) -> bool,
    {
        // Subscribe before the first check, so a commit between a check and
        // the select below still wakes the wait.
        let mut commits = self.wallet.subscribe_commits();
        let mut status = self.status.clone();
        loop {
            let holds = done(&*self.wallet.inner().read().await);
            if holds {
                return Ok(());
            }
            tokio::select! {
                Ok(()) = commits.changed() => {}
                ended = status.changed() => {
                    if ended.is_err() {
                        return Err(stopped());
                    }
                }
            }
        }
    }
}

impl std::fmt::Debug for BackgroundSync {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BackgroundSync")
            .field("finished", &self.task.is_finished())
            .field("last_error", &self.last_error())
            .finish_non_exhaustive()
    }
}

impl Drop for BackgroundSync {
    fn drop(&mut self) {
        // Dropping a `JoinHandle` only detaches its task.
        self.task.abort();
    }
}

/// Start the task of [`LocalWallet::sync_in_background`].
pub(crate) fn spawn(
    wallet: Arc<LocalWallet>,
    chain: Arc<dyn ChainView>,
    pause: Duration,
) -> BackgroundSync {
    let (status, receiver) = watch::channel(None);
    BackgroundSync {
        task: tokio::spawn(resync_until_aborted(
            Arc::clone(&wallet),
            chain,
            pause,
            status,
        )),
        status: receiver,
        wallet,
    }
}

async fn resync_until_aborted(
    wallet: Arc<LocalWallet>,
    chain: Arc<dyn ChainView>,
    pause: Duration,
    status: watch::Sender<Option<Arc<WalletError>>>,
) {
    // A failure that repeats logs at debug, so a permanent error such as a
    // replaced chain does not warn at every pause.
    let mut warned: Option<String> = None;
    loop {
        tokio::time::sleep(pause).await;
        match wallet.resync(&*chain).await {
            Ok(()) => {
                if warned.take().is_some() {
                    info!("background resync committed after a failed pass");
                }
                status.send_replace(None);
            }
            Err(error) => {
                let text = error.to_string();
                if warned.as_deref() == Some(text.as_str()) {
                    debug!(error = %text, "background resync failed again");
                } else {
                    warn!(error = %text, "background resync failed; the next pass runs after the pause");
                    warned = Some(text);
                }
                status.send_replace(Some(Arc::new(error)));
            }
        }
    }
}

/// The error of a background sync whose task has ended.
fn stopped() -> WalletError {
    WalletError::SyncTaskJoin(
        "the background sync stopped: a pass panicked or the runtime shut down".into(),
    )
}
