use std::sync::Arc;

use midnight_indexer_client::subscription::queries::UNSHIELDED_TRANSACTIONS_SUBSCRIPTION;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::state::{UnshieldedTxEvent, WalletState};

/// Background subscription that keeps wallet state updated via the indexer.
///
/// Subscribes to `unshieldedTransactions` for the wallet's address and applies
/// incoming events to the shared `WalletState`. Automatically reconnects on
/// connection failures.
pub struct WalletSync {
    state: Arc<RwLock<WalletState>>,
    cancel: CancellationToken,
    handle: JoinHandle<()>,
}

impl WalletSync {
    /// Spawn a background task that subscribes to the indexer and keeps
    /// the wallet state updated.
    pub fn spawn(state: Arc<RwLock<WalletState>>, address: String) -> Self {
        let cancel = CancellationToken::new();
        let token = cancel.clone();
        let sync_state = state.clone();

        let handle = tokio::spawn(async move {
            loop {
                if token.is_cancelled() {
                    break;
                }

                let (indexer_url, last_tx_id) = {
                    let s = sync_state.read().await;
                    (s.indexer_url().to_string(), s.last_tx_id())
                };

                if indexer_url.is_empty() {
                    debug!("no indexer URL configured, background sync disabled");
                    break;
                }

                let sub_client = midnight_indexer_client::SubscriptionClient::new(&indexer_url);

                let tx_id_str = last_tx_id
                    .map(|id| id.to_string())
                    .unwrap_or_else(|| "0".to_string());

                let variables = serde_json::json!({
                    "address": address.clone(),
                    "transactionId": tx_id_str,
                });

                let subscription = sub_client
                    .subscribe::<UnshieldedTxEvent>(UNSHIELDED_TRANSACTIONS_SUBSCRIPTION, variables)
                    .await;

                let mut subscription = match subscription {
                    Ok(s) => s,
                    Err(e) => {
                        warn!(error = %e, "failed to subscribe, retrying in 5s");
                        tokio::select! {
                            _ = token.cancelled() => break,
                            _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => continue,
                        }
                    }
                };

                info!("background sync subscription connected");

                loop {
                    tokio::select! {
                        _ = token.cancelled() => break,
                        event = subscription.next() => {
                            match event {
                                Some(Ok(ev)) => {
                                    let mut guard = sync_state.write().await;
                                    guard.apply_event(&ev);
                                    debug!("applied indexer event");
                                }
                                Some(Err(e)) => {
                                    warn!(error = %e, "subscription error");
                                    break;
                                }
                                None => {
                                    debug!("subscription ended, reconnecting");
                                    break;
                                }
                            }
                        }
                    }
                }

                if token.is_cancelled() {
                    break;
                }

                // Brief delay before reconnecting
                tokio::select! {
                    _ = token.cancelled() => break,
                    _ = tokio::time::sleep(std::time::Duration::from_secs(2)) => {}
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

    pub fn cancel(self) {
        self.cancel.cancel();
    }

    pub async fn shutdown(self) {
        self.cancel.cancel();
        let _ = self.handle.await;
    }
}
