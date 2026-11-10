//! [`Wallet`]'s builds.

use std::sync::Arc;

use async_trait::async_trait;
use midnight_helpers::{
    BuildContext, DefaultDB, FinalizedTransaction, ProofProvider, StandardTransactionInfo,
};
use midnight_types::convert::IntoLedger;
use midnight_types::{
    Cover, PreparedTransfer, TransferBuilder, TransferRequest, WalletError, balance_external,
    prepare_no_validate,
};
use midnight_wallet_facade::{ReservedBuild, WalletBuilds};

use crate::Wallet;

/// Reserve what `prepared` spends, and hand it over as reserved.
fn reserve(wallet: &mut crate::state::Wallet, prepared: PreparedTransfer) -> ReservedBuild {
    wallet.reserve_pending(
        prepared.dust_batches().to_vec(),
        prepared.spent_unshielded_inputs().to_vec(),
        prepared.spent_shielded_inputs().to_vec(),
        prepared.reserved_at(),
        prepared.expires_at(),
    );
    ReservedBuild::reserved(prepared)
}

/// Fund `tx_info` from `wallet`, and name `wallet`'s seed as the one the
/// build draws on, as [`crate::state::Wallet::add_funding`] requires.
fn fund_from(
    wallet: &mut crate::state::Wallet,
    tx_info: &mut StandardTransactionInfo<DefaultDB, BuildContext>,
) -> Result<(), WalletError> {
    wallet.add_funding(&tx_info.context)?;
    tx_info.set_funding_seeds(vec![wallet.seed().into_ledger()]);
    Ok(())
}

/// Refuse a pinned coin that a pending build of `wallet` holds.
///
/// The funding view leaves such a coin out, so a check after
/// [`fund_from`] could not tell it from a coin that the wallet does not own.
fn refuse_reserved(
    wallet: &crate::state::Wallet,
    pins: &[midnight_types::Nullifier],
) -> Result<(), WalletError> {
    for pin in pins.iter().map(|pin| pin.into_ledger()) {
        if wallet
            .reserved_shielded_nullifiers()
            .any(|held| *held == pin)
        {
            return Err(WalletError::InputsReserved {
                held: format!("shielded coin {pin:?}"),
            });
        }
    }
    Ok(())
}

#[async_trait]
impl WalletBuilds for Wallet {
    async fn execution_context(&self) -> Result<Arc<BuildContext>, WalletError> {
        let wallet = self.state().read().await;
        wallet.state().execution_context()
    }

    async fn prepare_transfer(
        &self,
        request: TransferRequest,
        proof_provider: Arc<dyn ProofProvider<DefaultDB>>,
    ) -> Result<ReservedBuild, WalletError> {
        let mut guard = self.state().write().await;
        let wallet = guard.state_mut();
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
        let mut guard = self.state().write().await;
        let wallet = guard.state_mut();
        fund_from(wallet, &mut tx_info)?;
        let prepared = prepare_no_validate(tx_info).await?;
        Ok(reserve(wallet, prepared))
    }

    async fn prepare_fees(
        &self,
        mut tx_info: StandardTransactionInfo<DefaultDB, BuildContext>,
        external: &FinalizedTransaction<DefaultDB>,
        cover: Cover,
    ) -> Result<Option<ReservedBuild>, WalletError> {
        let mut guard = self.state().write().await;
        let wallet = guard.state_mut();
        if let Cover::Shortfalls { pins, .. } = &cover {
            refuse_reserved(wallet, pins)?;
        }
        fund_from(wallet, &mut tx_info)?;
        let Some(prepared) = balance_external(tx_info, wallet.seed(), external, &cover)? else {
            return Ok(None);
        };
        Ok(Some(reserve(wallet, prepared)))
    }
}
