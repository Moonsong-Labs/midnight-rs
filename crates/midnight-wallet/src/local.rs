//! [`LocalWallet`]: the [`WalletFacade`] implementation for a [`Wallet`] this
//! process owns. Its builds for each ledger generation are in the generation
//! modules.

use async_trait::async_trait;
use midnight_types::chain_pin::{ChainCheck, ChainView, current_pin, verify_pin};
use midnight_types::{
    ChainParameters, CoinInfo, CoinPublicKey, EncryptionPublicKey, LedgerVersion, Network,
    SpendableShieldedCoin, SpentInputs, SyncCursors, TrackedUtxo, WalletBalance, WalletError,
    WalletSeed,
};
use midnight_wallet_facade::WalletFacade;
use tokio::sync::RwLock;
use tracing::warn;

use crate::Wallet;

/// A [`Wallet`] this process owns, shared behind its own lock.
///
/// The lock is a private field and no method hands it out, so a consumer that
/// holds a `LocalWallet` cannot block a sync by keeping a guard alive.
pub struct LocalWallet {
    inner: RwLock<Wallet>,
}

impl LocalWallet {
    pub fn new(wallet: Wallet) -> Self {
        Self {
            inner: RwLock::new(wallet),
        }
    }
}

impl LocalWallet {
    pub(crate) fn inner(&self) -> &RwLock<Wallet> {
        &self.inner
    }
}

impl From<Wallet> for LocalWallet {
    fn from(wallet: Wallet) -> Self {
        Self::new(wallet)
    }
}

#[async_trait]
impl WalletFacade for LocalWallet {
    async fn network(&self) -> Network {
        Network::from(self.inner.read().await.network())
    }

    async fn ledger_version(&self) -> LedgerVersion {
        self.inner.read().await.ledger_version()
    }

    async fn seed(&self) -> WalletSeed {
        self.inner.read().await.seed().clone()
    }

    async fn shielded_public_keys(&self) -> (CoinPublicKey, EncryptionPublicKey) {
        self.inner.read().await.shielded_public_keys()
    }

    async fn balance(&self) -> WalletBalance {
        self.inner.read().await.balance()
    }

    async fn spendable_shielded_coins(&self) -> Vec<SpendableShieldedCoin> {
        self.inner.read().await.spendable_shielded_coins()
    }

    async fn unshielded_utxos(&self) -> Vec<TrackedUtxo> {
        self.inner.read().await.unshielded_utxos().to_vec()
    }

    async fn parameters(&self) -> ChainParameters {
        self.inner.read().await.parameters()
    }

    async fn sync_cursors(&self) -> SyncCursors {
        self.inner.read().await.sync_cursors()
    }

    async fn dust_synced(&self) -> bool {
        self.inner.read().await.dust_synced()
    }

    async fn release(&self, spent: &SpentInputs) {
        self.inner.write().await.release(spent);
    }

    async fn has_observed(&self, spent: &[SpentInputs]) -> bool {
        self.inner.read().await.has_observed(spent)
    }

    async fn resync(&self, chain: &dyn ChainView) -> Result<(), WalletError> {
        let (pin, snapshot, indexer_url) = {
            let wallet = self.inner.read().await;
            (
                wallet.chain_pin().cloned(),
                wallet.snapshot_dir(),
                wallet.indexer_url().to_string(),
            )
        };

        // Take the replacement pin before the check, not after the commit. It
        // is the mark this resync's state belongs to, and a chain replaced at
        // any point after this reads as replaced next time. Taken afterwards,
        // a swap during the resync would be stamped with the new chain's own
        // block, and every later check would pass against a chain this state
        // never saw.
        let replacement = if pin.is_some() {
            current_pin(chain).await
        } else {
            None
        };

        if let Some(pin) = &pin {
            match verify_pin(chain, pin).await {
                ChainCheck::SameChain => {}
                // A node that cannot answer leaves the wallet alone: a pruned
                // archive must not condemn a healthy one.
                ChainCheck::Unknown => warn!(
                    height = pin.height,
                    "node could not answer for the pinned block; keeping the cached state"
                ),
                ChainCheck::Replaced { found } => {
                    return Err(WalletError::ChainMismatch {
                        path: snapshot
                            .map(|p| p.display().to_string())
                            .unwrap_or_else(|| "the wallet snapshot".to_string()),
                        pinned_height: pin.height,
                        pinned_hash: pin.hash.clone(),
                        found: found.unwrap_or_else(|| "no block".to_string()),
                    });
                }
            }
        }

        // The plan is snapshotted under a read lock and the commit applied
        // under a write one, so the replay in between runs with the wallet
        // free.
        let plan = self.inner.read().await.resync_plan();
        let commit = plan.run(&indexer_url).await?;
        self.inner.write().await.commit_resync(commit)?;

        // Move the pin forward with the commit, so a wallet that runs for a
        // long time keeps a recent mark rather than one an archive has since
        // pruned, which would leave every later check inconclusive.
        if let Some(fresh) = replacement {
            self.inner.write().await.set_chain_pin(fresh);
        }
        Ok(())
    }

    async fn rescan_shielded(&self) -> Result<(), WalletError> {
        let (plan, indexer_url) = {
            let wallet = self.inner.read().await;
            (
                wallet.shielded_rescan_plan(),
                wallet.indexer_url().to_string(),
            )
        };
        let commit = plan.run(&indexer_url).await?;
        self.inner.write().await.commit_shielded_rescan(commit)
    }

    async fn watch_for_coins(&self, coins: Vec<CoinInfo>) -> Result<(), WalletError> {
        self.inner.write().await.watch_for_coins(coins)
    }

    async fn forget_coins(&self, coins: Vec<CoinInfo>) -> Result<(), WalletError> {
        self.inner.write().await.forget_coins(coins)
    }
}
