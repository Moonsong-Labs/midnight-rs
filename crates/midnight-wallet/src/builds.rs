//! [`Wallet`]'s funding and signing, as a [`MidnightWallet`].

use async_trait::async_trait;
use midnight_helpers::{BuildContext, DefaultDB, StandardTransactionInfo};
use midnight_types::build::{
    self, Cover, Fill, Funded, PreparedTransfer, SpentInputs, TransferBuilder, balance_external,
    prepare_no_validate,
};
use midnight_types::chain_pin::ChainView;
use midnight_types::convert::IntoLedger;
use midnight_types::{
    FilledTx, FundedTx, MidnightWallet, ProvenTx, ShieldedRecipient, TxEnvelope, WalletError,
};

use crate::Wallet;
use crate::pending;

/// Reserve what `prepared` spends in `wallet`'s book.
fn reserve(wallet: &mut crate::state::Wallet, prepared: &PreparedTransfer) -> SpentInputs {
    wallet.reserve_pending(
        prepared.dust_batches().to_vec(),
        prepared.spent_unshielded_inputs().to_vec(),
        prepared.spent_shielded_inputs().to_vec(),
        prepared.reserved_at(),
        prepared.expires_at(),
    );
    prepared.spent_inputs()
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
    let held = wallet.reserved_shielded_nullifiers();
    for pin in pins.iter().map(|pin| pin.into_ledger()) {
        if held.contains(&pin) {
            return Err(WalletError::InputsReserved {
                held: format!("shielded coin {pin:?}"),
            });
        }
    }
    Ok(())
}

/// Fund `fill` from `wallet`, and reserve what it spends.
///
/// Selection, assembly, the fee loop and the reservation all run here, under
/// the one write lock the caller holds, so two fundings cannot draw the same
/// input. On an error, nothing is reserved.
async fn fund(
    wallet: &mut crate::state::Wallet,
    fill: Fill,
) -> Result<(Funded, SpentInputs), WalletError> {
    let prepared = match fill {
        Fill::Transfer {
            request,
            context,
            prover,
        } => {
            wallet.add_funding(&context)?;
            TransferBuilder::new(&*wallet, context, prover)
                .prepare(request)
                .await?
        }
        Fill::Assembled(mut tx_info) => {
            fund_from(wallet, &mut tx_info)?;
            prepare_no_validate(*tx_info).await?
        }
        Fill::Sealed {
            mut tx_info,
            external,
            cover,
        } => {
            if let Cover::Shortfalls { pins, .. } = &cover {
                refuse_reserved(wallet, pins)?;
            }
            fund_from(wallet, &mut tx_info)?;
            match balance_external(*tx_info, wallet.seed(), &external, &cover)? {
                Some(prepared) => prepared,
                None => return Ok((Funded::Unchanged(external), SpentInputs::default())),
            }
        }
    };
    let spent = reserve(wallet, &prepared);
    Ok((Funded::Prepared(Box::new(prepared)), spent))
}

#[async_trait]
impl MidnightWallet for Wallet {
    fn shielded_recipient(&self) -> ShieldedRecipient {
        Wallet::shielded_recipient(self)
    }

    async fn fund_transaction(
        &self,
        chain: &dyn ChainView,
        tx: FilledTx,
    ) -> Result<FundedTx, WalletError> {
        // Boxed: a debug build makes the replay future tens of kilobytes, and
        // every funding awaits it.
        Box::pin(self.sync(chain)).await?;
        let (funded, spent) = {
            let mut guard = self.state().write().await;
            fund(guard.state_mut(), build::fill_of(tx)).await?
        };
        let reservation = pending::reservation(self.pending(), spent);
        Ok(build::funded(funded, reservation))
    }

    /// Serialize `tx`, then keep its reservation across a restart.
    ///
    /// The fund step signed `tx` already, and the proof leaves those
    /// signatures valid, so this adds none.
    async fn sign_transaction(&self, tx: ProvenTx) -> Result<TxEnvelope, WalletError> {
        let spent = build::spent_by(&tx).clone();
        let envelope = build::envelope(tx)?;
        pending::lock(self.pending()).mark_proven(&spent);
        Ok(envelope)
    }
}
