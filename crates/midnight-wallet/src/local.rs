//! [`LocalWallet`]: the [`WalletFacade`] implementation for a [`Wallet`] this
//! process owns.

use std::sync::Arc;

use async_trait::async_trait;
use midnight_types::chain_pin::{ChainCheck, ChainView, current_pin, verify_pin};
use midnight_types::{
    BuilderCtx, CoinInfo, CoinPublicKey, DefaultDB, EncryptionPublicKey, FinalizedTransaction,
    LedgerContext, LedgerParameters, ProofProvider, StandardTrasactionInfo, WalletSeed,
};
use midnight_types::{
    Network, SpendableShieldedCoin, SpentInputs, SyncCursors, TrackedUtxo, TransferRequest,
    WalletBalance, WalletError,
};
use midnight_wallet_facade::{ReservedBuild, WalletFacade};
use tokio::sync::RwLock;
use tracing::warn;

use crate::state::Wallet;
use crate::transfer::{TransferBuilder, balance_external, prepare_no_validate};

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
        let prepared = TransferBuilder::new(&*wallet, context, proof_provider)
            .prepare(request)
            .await?;
        let spent = prepared.spent_inputs();
        wallet.reserve_pending(
            spent.dust_batches,
            spent.unshielded,
            spent.shielded,
            spent.reserved_at,
        );
        Ok(ReservedBuild::reserved(prepared))
    }

    async fn prepare_funded(
        &self,
        mut tx_info: StandardTrasactionInfo<DefaultDB, BuilderCtx>,
    ) -> Result<ReservedBuild, WalletError> {
        let mut wallet = self.inner.write().await;
        wallet.add_funding(&tx_info.context)?;
        tx_info.set_funding_seeds(vec![wallet.seed().clone()]);
        let prepared = prepare_no_validate(tx_info).await?;
        let spent = prepared.spent_inputs();
        wallet.reserve_pending(
            spent.dust_batches,
            spent.unshielded,
            spent.shielded,
            spent.reserved_at,
        );
        Ok(ReservedBuild::reserved(prepared))
    }

    async fn prepare_fees(
        &self,
        mut tx_info: StandardTrasactionInfo<DefaultDB, BuilderCtx>,
        external: &FinalizedTransaction<DefaultDB>,
    ) -> Result<Option<ReservedBuild>, WalletError> {
        let mut wallet = self.inner.write().await;
        wallet.add_funding(&tx_info.context)?;
        tx_info.set_funding_seeds(vec![wallet.seed().clone()]);
        let Some(prepared) = balance_external(tx_info, external)? else {
            return Ok(None);
        };
        let spent = prepared.spent_inputs();
        wallet.reserve_pending(
            spent.dust_batches,
            spent.unshielded,
            spent.shielded,
            spent.reserved_at,
        );
        Ok(Some(ReservedBuild::reserved(prepared)))
    }

    async fn spend_shielded(
        &self,
        context: &Arc<LedgerContext<DefaultDB>>,
        nullifiers: Vec<midnight_types::Nullifier>,
        rng: &mut midnight_types::StdRng,
    ) -> Result<(Vec<midnight_types::PreparedInput>, SpentInputs), WalletError> {
        // Nothing to spend, so nothing to hold the wallet or rewrite the
        // pending file for.
        if nullifiers.is_empty() {
            return Ok((Vec::new(), SpentInputs::default()));
        }
        let mut wallet = self.inner.write().await;
        // The funding view in `context` predates this hold, so a coin named
        // here can have been reserved since.
        if let Some(taken) = nullifiers
            .iter()
            .find(|n| wallet.reserved_shielded_nullifiers().any(|held| held == *n))
        {
            return Err(WalletError::InputsReserved {
                held: format!("shielded coin {taken:?}"),
            });
        }

        let prepared = midnight_types::prepared_input::prepare_shielded_inputs(
            context,
            wallet.seed(),
            &nullifiers,
            rng,
        )?;
        let spent = SpentInputs::from_shielded(nullifiers, context.latest_block_context().tblock);
        wallet.reserve_pending(
            Vec::new(),
            Vec::new(),
            spent.shielded.clone(),
            spent.reserved_at,
        );
        Ok((prepared, spent))
    }

    async fn release(&self, spent: &SpentInputs) {
        self.inner.write().await.release_pending(
            &spent.dust_nullifiers(),
            &spent.unshielded,
            &spent.shielded,
            spent.reserved_at,
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
