//! The pipeline of a transaction that the provider's wallet funds: the
//! provider fills it from node queries, the wallet funds it, the provider
//! proves it, and the wallet signs it.

use std::sync::Arc;

use midnight_helpers::midnight_serialize::{tagged_deserialize, tagged_serialize};
use midnight_helpers::{
    BuildContext, DefaultDB, FinalizedTransaction, Timestamp, make_block_context,
};
use midnight_types::build::{self, Fill, TransferRequest};
use midnight_types::{FilledTx, MidnightWallet, TxEnvelope};

use crate::{MidnightProvider, ProviderError};

impl<W> MidnightProvider<W> {
    /// The half of a [`BuildContext`] that a transaction executes against,
    /// from node queries: the node's network and ledger parameters, the
    /// resolver, and the time of the node's best block.
    ///
    /// A transaction built on it counts its TTL from that time. The result
    /// holds no wallet, so it funds nothing.
    #[doc(hidden)]
    pub async fn chain_context(&self) -> Result<Arc<BuildContext>, ProviderError> {
        let best_block_time = async {
            self.get_block_timestamp(self.best_block_hash().await?)
                .await
        };
        let (network, parameters, time) = tokio::try_join!(
            self.ledger_network_id(),
            self.ledger_parameters(),
            best_block_time
        )?;
        let time = Timestamp::from_secs(time.as_secs());
        Ok(midnight_types::build::execution_context(
            network,
            parameters,
            Some(make_block_context(time, Default::default(), time)),
        )?)
    }
}

impl<W: MidnightWallet> MidnightProvider<W> {
    /// Fund `tx` with the wallet, prove it with this provider's prover, and
    /// let the wallet sign it.
    ///
    /// A step that fails drops the reservation that the funding made, which
    /// hands the inputs back to the wallet.
    #[doc(hidden)]
    pub async fn fund_prove_sign(&self, tx: FilledTx) -> Result<TxEnvelope, ProviderError> {
        // Boxed so the wallet's resync stays off the caller's frame. A debug
        // build makes this future tens of kilobytes, and an inlined one is
        // carried by every future that awaits it, up to the test or task that
        // owns the stack. Every build path awaits this one.
        Box::pin(self.fund_prove_sign_inner(tx)).await
    }

    async fn fund_prove_sign_inner(&self, tx: FilledTx) -> Result<TxEnvelope, ProviderError> {
        let funded = self.wallet().fund_transaction(self, tx).await?;
        let proven = build::prove(funded).await?;
        Ok(self.wallet().sign_transaction(proven).await?)
    }

    /// Fill the transfer that `request` names, then fund, prove and sign it.
    pub(crate) async fn build_transfer(
        &self,
        request: TransferRequest,
    ) -> Result<TxEnvelope, ProviderError> {
        let fill = Fill::Transfer {
            request,
            context: self.chain_context().await?,
            prover: self.proof_provider(),
        };
        self.fund_prove_sign(build::filled(fill)).await
    }
}

/// Merge proven transactions into one. See
/// [`MidnightProvider::merge_transactions`].
pub(crate) fn merge_transactions(txs: &[Vec<u8>]) -> Result<Vec<u8>, ProviderError> {
    let mut iter = txs.iter();
    let first = iter.next().ok_or_else(|| {
        ProviderError::Transaction("merge_transactions requires at least one transaction".into())
    })?;
    let mut merged = deserialize_transaction(first)?;
    for bytes in iter {
        let other = deserialize_transaction(bytes)?;
        merged = merged
            .merge(&other)
            .map_err(|e| ProviderError::Transaction(format!("merge transactions: {e:?}")))?;
    }

    let mut out = Vec::new();
    tagged_serialize(&merged, &mut out)
        .map_err(|e| ProviderError::Transaction(format!("serialize merged transaction: {e}")))?;
    Ok(out)
}

pub(crate) fn deserialize_transaction(
    bytes: &[u8],
) -> Result<FinalizedTransaction<DefaultDB>, ProviderError> {
    tagged_deserialize(&mut &bytes[..])
        .map_err(|e| ProviderError::Transaction(format!("deserialize transaction: {e}")))
}
