//! [`LocalWallet`]: the [`WalletFacade`] implementation for a [`Wallet`] this
//! process owns.

use std::sync::Arc;

use async_trait::async_trait;
use midnight_helpers::{
    CoinInfo, CoinPublicKey, DefaultDB, EncryptionPublicKey, LedgerContext, LedgerParameters,
    ProofProvider, Timestamp, WalletSeed,
};
use midnight_types::chain_pin::{ChainCheck, ChainView, current_pin, verify_pin};
use midnight_types::{
    Network, SpendableShieldedCoin, SpentInputs, SyncCursors, TrackedUtxo, TransferRequest,
    WalletBalance, WalletError,
};
use midnight_wallet_facade::{ReservedBuild, WalletFacade};
use tokio::sync::RwLock;
use tracing::warn;

use crate::state::Wallet;
use crate::transfer::TransferBuilder;

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

    async fn parameters(&self) -> LedgerParameters {
        self.inner.read().await.parameters().clone()
    }

    async fn sync_cursors(&self) -> SyncCursors {
        self.inner.read().await.sync_cursors()
    }

    async fn dust_synced(&self) -> bool {
        self.inner.read().await.dust_synced()
    }

    async fn execution_context(&self) -> Result<Arc<LedgerContext<DefaultDB>>, WalletError> {
        self.inner.read().await.execution_context()
    }

    async fn add_funding(&self, context: &LedgerContext<DefaultDB>) -> Result<(), WalletError> {
        self.inner.write().await.add_funding(context)
    }

    async fn prepare_transfer(
        &self,
        request: TransferRequest,
        proof_provider: Arc<dyn ProofProvider<DefaultDB>>,
    ) -> Result<ReservedBuild, WalletError> {
        let mut wallet = self.inner.write().await;
        let context = wallet.build_context_inner()?;
        let reserved_at = context.latest_block_context().tblock;
        let prepared = TransferBuilder::new(&*wallet, context, proof_provider)
            .prepare(request)
            .await?;
        let spent = prepared.spent_inputs();
        wallet.reserve_pending(
            spent.dust_batches,
            spent.unshielded,
            spent.shielded,
            reserved_at,
        );
        Ok(ReservedBuild::reserved(prepared))
    }

    async fn reserve(&self, spent: SpentInputs, reserved_at: Timestamp) {
        self.inner.write().await.reserve_pending(
            spent.dust_batches,
            spent.unshielded,
            spent.shielded,
            reserved_at,
        );
    }

    async fn release(&self, spent: &SpentInputs) {
        self.inner.write().await.release_pending(
            &spent.dust_nullifiers(),
            &spent.unshielded,
            &spent.shielded,
        );
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
        if pin.is_some()
            && let Some(fresh) = current_pin(chain).await
        {
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
