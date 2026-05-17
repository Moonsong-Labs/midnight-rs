use std::sync::Arc;

use midnight_indexer_client::subscription::queries::{
    DUST_LEDGER_EVENTS_SUBSCRIPTION, UNSHIELDED_TRANSACTIONS_SUBSCRIPTION,
    ZSWAP_LEDGER_EVENTS_SUBSCRIPTION,
};
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::state::{
    DustEventEnvelope, UnshieldedTxEvent, WalletState, ZswapEventEnvelope,
};

/// Background subscription that keeps wallet state updated via the indexer.
///
/// Maintains three concurrent subscriptions:
/// - `zswapLedgerEvents` for shielded coin tracking
/// - `dustLedgerEvents` for dust/fee tracking
/// - `unshieldedTransactions` for unshielded UTXO balance
///
/// Each subscription reconnects independently on failure using its last-seen
/// event ID as the resume cursor.
pub struct WalletSync {
    state: Arc<RwLock<WalletState>>,
    cancel: CancellationToken,
    handle: JoinHandle<()>,
}

impl WalletSync {
    pub fn spawn(state: Arc<RwLock<WalletState>>, address: String) -> Self {
        let cancel = CancellationToken::new();
        let token = cancel.clone();
        let sync_state = state.clone();

        let handle = tokio::spawn(async move {
            let zswap_token = token.child_token();
            let dust_token = token.child_token();
            let unshielded_token = token.child_token();

            let zswap_state = sync_state.clone();
            let dust_state = sync_state.clone();
            let unshielded_state = sync_state.clone();
            let address_clone = address.clone();

            tokio::join!(
                run_zswap_sync(zswap_state, zswap_token),
                run_dust_sync(dust_state, dust_token),
                run_unshielded_sync(unshielded_state, unshielded_token, address_clone),
            );
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

async fn run_zswap_sync(state: Arc<RwLock<WalletState>>, token: CancellationToken) {
    loop {
        if token.is_cancelled() {
            break;
        }

        let (indexer_url, last_id) = {
            let s = state.read().await;
            (s.indexer_url().to_string(), s.zswap_event_id())
        };

        if indexer_url.is_empty() {
            debug!("no indexer URL, zswap sync disabled");
            break;
        }

        let sub_client = midnight_indexer_client::SubscriptionClient::new(&indexer_url);
        let variables = serde_json::json!({ "id": last_id });

        let subscription = tokio::select! {
            _ = token.cancelled() => break,
            result = tokio::time::timeout(
                std::time::Duration::from_secs(15),
                sub_client.subscribe::<ZswapEventEnvelope>(
                    ZSWAP_LEDGER_EVENTS_SUBSCRIPTION,
                    variables,
                ),
            ) => result,
        };

        let mut subscription = match subscription {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => {
                warn!(error = %e, "zswap subscribe failed, retrying in 5s");
                tokio::select! {
                    _ = token.cancelled() => break,
                    _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => continue,
                }
            }
            Err(_) => {
                warn!("zswap subscribe timed out, retrying in 5s");
                tokio::select! {
                    _ = token.cancelled() => break,
                    _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => continue,
                }
            }
        };

        info!("zswap background sync connected");

        loop {
            tokio::select! {
                _ = token.cancelled() => break,
                event = subscription.next() => {
                    match event {
                        Some(Ok(envelope)) => {
                            let mut guard = state.write().await;
                            if let Err(e) = guard.apply_zswap_event(&envelope.zswap_ledger_events) {
                                warn!(error = %e, "failed to apply zswap event");
                                break;
                            }
                            debug!(id = envelope.zswap_ledger_events.id, "applied zswap event");
                        }
                        Some(Err(e)) => {
                            warn!(error = %e, "zswap subscription error");
                            break;
                        }
                        None => {
                            debug!("zswap subscription ended, reconnecting");
                            break;
                        }
                    }
                }
            }
        }

        if token.is_cancelled() {
            break;
        }
        tokio::select! {
            _ = token.cancelled() => break,
            _ = tokio::time::sleep(std::time::Duration::from_secs(2)) => {}
        }
    }
}

async fn run_dust_sync(state: Arc<RwLock<WalletState>>, token: CancellationToken) {
    loop {
        if token.is_cancelled() {
            break;
        }

        let (indexer_url, last_id) = {
            let s = state.read().await;
            (s.indexer_url().to_string(), s.dust_event_id())
        };

        if indexer_url.is_empty() {
            debug!("no indexer URL, dust sync disabled");
            break;
        }

        let sub_client = midnight_indexer_client::SubscriptionClient::new(&indexer_url);
        let variables = serde_json::json!({ "id": last_id });

        let subscription = tokio::select! {
            _ = token.cancelled() => break,
            result = tokio::time::timeout(
                std::time::Duration::from_secs(15),
                sub_client.subscribe::<DustEventEnvelope>(
                    DUST_LEDGER_EVENTS_SUBSCRIPTION,
                    variables,
                ),
            ) => result,
        };

        let mut subscription = match subscription {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => {
                warn!(error = %e, "dust subscribe failed, retrying in 5s");
                tokio::select! {
                    _ = token.cancelled() => break,
                    _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => continue,
                }
            }
            Err(_) => {
                warn!("dust subscribe timed out, retrying in 5s");
                tokio::select! {
                    _ = token.cancelled() => break,
                    _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => continue,
                }
            }
        };

        info!("dust background sync connected");

        loop {
            tokio::select! {
                _ = token.cancelled() => break,
                event = subscription.next() => {
                    match event {
                        Some(Ok(envelope)) => {
                            let mut guard = state.write().await;
                            if let Err(e) = guard.apply_dust_event(&envelope.dust_ledger_events) {
                                warn!(error = %e, "failed to apply dust event");
                                break;
                            }
                            debug!(id = envelope.dust_ledger_events.id, "applied dust event");
                        }
                        Some(Err(e)) => {
                            warn!(error = %e, "dust subscription error");
                            break;
                        }
                        None => {
                            debug!("dust subscription ended, reconnecting");
                            break;
                        }
                    }
                }
            }
        }

        if token.is_cancelled() {
            break;
        }
        tokio::select! {
            _ = token.cancelled() => break,
            _ = tokio::time::sleep(std::time::Duration::from_secs(2)) => {}
        }
    }
}

async fn run_unshielded_sync(
    state: Arc<RwLock<WalletState>>,
    token: CancellationToken,
    address: String,
) {
    loop {
        if token.is_cancelled() {
            break;
        }

        let (indexer_url, last_tx_id) = {
            let s = state.read().await;
            (s.indexer_url().to_string(), s.last_tx_id())
        };

        if indexer_url.is_empty() {
            debug!("no indexer URL, unshielded sync disabled");
            break;
        }

        let sub_client = midnight_indexer_client::SubscriptionClient::new(&indexer_url);
        let variables = serde_json::json!({
            "address": address,
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
                warn!(error = %e, "unshielded subscribe failed, retrying in 5s");
                tokio::select! {
                    _ = token.cancelled() => break,
                    _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => continue,
                }
            }
            Err(_) => {
                warn!("unshielded subscribe timed out, retrying in 5s");
                tokio::select! {
                    _ = token.cancelled() => break,
                    _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => continue,
                }
            }
        };

        info!("unshielded background sync connected");

        loop {
            tokio::select! {
                _ = token.cancelled() => break,
                event = subscription.next() => {
                    match event {
                        Some(Ok(ev)) => {
                            let mut guard = state.write().await;
                            guard.apply_unshielded_event(&ev);
                            debug!("applied unshielded event");
                        }
                        Some(Err(e)) => {
                            warn!(error = %e, "unshielded subscription error");
                            break;
                        }
                        None => {
                            debug!("unshielded subscription ended, reconnecting");
                            break;
                        }
                    }
                }
            }
        }

        if token.is_cancelled() {
            break;
        }
        tokio::select! {
            _ = token.cancelled() => break,
            _ = tokio::time::sleep(std::time::Duration::from_secs(2)) => {}
        }
    }
}
