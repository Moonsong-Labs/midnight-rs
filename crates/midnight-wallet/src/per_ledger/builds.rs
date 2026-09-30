//! [`LocalWallet`]'s builds on this generation.

use std::sync::Arc;

use async_trait::async_trait;
use midnight_types::{Nullifier, SpentInputs, TransferRequest, WalletError};

use super::facade::{ReservedBuild, WalletBuilds};
use super::helpers::{
    BuildContext, DefaultDB, FinalizedTransaction, ProofProvider, StandardTransactionInfo, StdRng,
};
use super::types::convert::IntoLedger;
use super::types::{
    PreparedInput, PreparedTransfer, TransferBuilder, balance_external, prepare_no_validate,
    prepare_shielded_inputs,
};
use crate::LocalWallet;

/// Reserve what `prepared` spends, and hand it over as reserved.
fn reserve(wallet: &mut super::Wallet, prepared: PreparedTransfer) -> ReservedBuild {
    let spent = prepared.spent_inputs();
    wallet.reserve_pending(
        prepared.dust_batches().to_vec(),
        spent.unshielded,
        prepared.spent_shielded_inputs().to_vec(),
        spent.reserved_at,
    );
    ReservedBuild::reserved(prepared)
}

#[async_trait]
impl WalletBuilds for LocalWallet {
    async fn execution_context(&self) -> Result<Arc<BuildContext>, WalletError> {
        let wallet = self.inner().read().await;
        super::wallet_state(&wallet)?.execution_context()
    }

    async fn add_funding(&self, context: &BuildContext) -> Result<(), WalletError> {
        let mut wallet = self.inner().write().await;
        super::wallet_state_mut(&mut wallet)?.add_funding(context)
    }

    async fn prepare_transfer(
        &self,
        request: TransferRequest,
        proof_provider: Arc<dyn ProofProvider<DefaultDB>>,
    ) -> Result<ReservedBuild, WalletError> {
        let mut guard = self.inner().write().await;
        let wallet = super::wallet_state_mut(&mut guard)?;
        let context = wallet.build_context_inner()?;
        let prepared = TransferBuilder::new(&*wallet, context, proof_provider)
            .prepare(request)
            .await?;
        Ok(reserve(wallet, prepared))
    }

    async fn prepare_funded(
        &self,
        mut tx_info: StandardTransactionInfo<DefaultDB, BuildContext>,
    ) -> Result<ReservedBuild, WalletError> {
        let mut guard = self.inner().write().await;
        let wallet = super::wallet_state_mut(&mut guard)?;
        wallet.add_funding(&tx_info.context)?;
        tx_info.set_funding_seeds(vec![wallet.seed().clone()]);
        let prepared = prepare_no_validate(tx_info).await?;
        Ok(reserve(wallet, prepared))
    }

    async fn spend_shielded(
        &self,
        context: &Arc<BuildContext>,
        nullifiers: Vec<Nullifier>,
        rng: &mut StdRng,
    ) -> Result<(Vec<PreparedInput>, SpentInputs), WalletError> {
        // Nothing to spend, so nothing to hold the wallet or rewrite the
        // pending file for.
        if nullifiers.is_empty() {
            return Ok((Vec::new(), SpentInputs::default()));
        }
        let mut guard = self.inner().write().await;
        let wallet = super::wallet_state_mut(&mut guard)?;
        let ledger_nullifiers: Vec<_> = nullifiers.iter().map(|n| n.into_ledger()).collect();
        // The funding view in `context` predates this hold, so a coin named
        // here can have been reserved since.
        if let Some(taken) = ledger_nullifiers
            .iter()
            .find(|n| wallet.reserved_shielded_nullifiers().any(|held| held == *n))
        {
            return Err(WalletError::InputsReserved {
                held: format!("shielded coin {taken:?}"),
            });
        }

        let prepared = prepare_shielded_inputs(context, wallet.seed(), &ledger_nullifiers, rng)?;
        let reserved_at = context.latest_block_context().tblock;
        wallet.reserve_pending(Vec::new(), Vec::new(), ledger_nullifiers, reserved_at);
        Ok((
            prepared,
            SpentInputs::from_shielded(nullifiers, reserved_at),
        ))
    }

    async fn prepare_fees(
        &self,
        mut tx_info: StandardTransactionInfo<DefaultDB, BuildContext>,
        external: &FinalizedTransaction<DefaultDB>,
    ) -> Result<Option<ReservedBuild>, WalletError> {
        let mut guard = self.inner().write().await;
        let wallet = super::wallet_state_mut(&mut guard)?;
        wallet.add_funding(&tx_info.context)?;
        tx_info.set_funding_seeds(vec![wallet.seed().clone()]);
        let Some(prepared) = balance_external(tx_info, external)? else {
            return Ok(None);
        };
        Ok(Some(reserve(wallet, prepared)))
    }
}
