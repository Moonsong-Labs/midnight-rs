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
            let mut last_seen_chain_height: Option<i64> = None;
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
                let indexer_client = match midnight_indexer_client::IndexerClient::new(&indexer_url)
                {
                    Ok(client) => Some(client),
                    Err(e) => {
                        warn!(
                            error = %e,
                            "failed to initialize HTTP indexer client for chain tip tracking"
                        );
                        None
                    }
                };

                let variables = serde_json::json!({
                    "address": address.clone(),
                    "transactionId": last_tx_id.unwrap_or(0),
                });

                let subscription = tokio::select! {
                    _ = token.cancelled() => break,
                    result = tokio::time::timeout(
                        std::time::Duration::from_secs(15),
                        sub_client.subscribe::<UnshieldedTxEvent>(
                            UNSHIELDED_TRANSACTIONS_SUBSCRIPTION,
                            variables,
                        ),
                    ) => result,
                };

                let mut subscription = match subscription {
                    Ok(Ok(s)) => s,
                    Ok(Err(e)) => {
                        warn!(error = %e, "failed to subscribe, retrying in 5s");
                        tokio::select! {
                            _ = token.cancelled() => break,
                            _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => continue,
                        }
                    }
                    Err(_) => {
                        warn!("subscribe timed out, retrying in 5s");
                        tokio::select! {
                            _ = token.cancelled() => break,
                            _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => continue,
                        }
                    }
                };

                info!("background sync subscription connected");
                let mut tip_poll = tokio::time::interval(std::time::Duration::from_secs(2));
                tip_poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

                loop {
                    tokio::select! {
                        _ = token.cancelled() => break,
                        _ = tip_poll.tick() => {
                            if let Some(client) = &indexer_client {
                                match client.get_block(None).await {
                                    Ok(Some(block)) => {
                                        let height = block.height;
                                        let prev = last_seen_chain_height;
                                        last_seen_chain_height = Some(height);
                                        if prev.map(|h| height > h).unwrap_or(true) {
                                            let mut guard = sync_state.write().await;
                                            guard.observe_chain_tip(height);
                                            debug!(height, "observed chain tip, refreshed context freshness state");
                                        }
                                    }
                                    Ok(None) => {}
                                    Err(e) => {
                                        warn!(error = %e, "failed to poll latest block for context freshness");
                                    }
                                }
                            }
                        }
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

    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    pub async fn shutdown(self) {
        self.cancel.cancel();
        let _ = self.handle.await;
    }
}
