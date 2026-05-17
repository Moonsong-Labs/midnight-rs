use std::sync::Arc;
use std::time::Duration;

use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use crate::state::WalletState;

pub struct WalletSync {
    state: Arc<RwLock<WalletState>>,
    cancel: CancellationToken,
    handle: JoinHandle<()>,
}

impl WalletSync {
    pub fn spawn(
        state: Arc<RwLock<WalletState>>,
        interval: Duration,
    ) -> Self {
        let cancel = CancellationToken::new();
        let token = cancel.clone();
        let sync_state = state.clone();

        let handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = token.cancelled() => break,
                    _ = tokio::time::sleep(interval) => {}
                }

                let mut guard = sync_state.write().await;
                match guard.resync().await {
                    Ok(result) => {
                        debug!(
                            height = result.height,
                            blocks = result.blocks_processed,
                            "background sync tick"
                        );
                    }
                    Err(e) => {
                        warn!(error = %e, "background sync failed");
                    }
                }
            }
        });

        Self {
            state,
            cancel,
            handle,
        }
    }

    pub fn state(&self) -> &Arc<RwLock<WalletState>> {
        &self.state
    }

    pub async fn shutdown(self) {
        self.cancel.cancel();
        let _ = self.handle.await;
    }
}
