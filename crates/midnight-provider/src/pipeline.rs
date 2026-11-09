//! The pipeline of a transaction that the provider's wallet funds: the
//! provider fills it from node queries, the wallet funds it, the provider
//! proves it, and the wallet signs it.

use std::sync::Arc;

use midnight_helpers::midnight_serialize::{tagged_deserialize, tagged_serialize};
use midnight_helpers::{
    BuildContext, DefaultDB, FinalizedTransaction, FromContext, StandardTransactionInfo, Timestamp,
    make_block_context,
};
use midnight_types::build;
use midnight_types::{FilledTx, MidnightWallet, TransactionRequest, TxEnvelope};

use crate::{MidnightProvider, PendingTx, ProviderError};

impl<W> MidnightProvider<W> {
    /// The half of a [`BuildContext`] that a transaction executes against,
    /// from node queries: the node's network and ledger parameters, the
    /// resolver, and the time of the node's best block.
    ///
    /// A transaction built on it counts its TTL from that time. The result
    /// holds no wallet, so it funds nothing.
    pub(crate) async fn chain_context(&self) -> Result<Arc<BuildContext>, ProviderError> {
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

    /// An empty transaction on the chain's context (see
    /// [`Self::chain_context`]) that this provider's prover proves, for
    /// midnight-contract to fill a [`ContractPart`](build::ContractPart) in.
    #[doc(hidden)]
    pub async fn transaction_info(
        &self,
    ) -> Result<StandardTransactionInfo<DefaultDB, BuildContext>, ProviderError> {
        Ok(StandardTransactionInfo::new_from_context(
            self.chain_context().await?,
            self.prover.proof_provider(),
            None,
        ))
    }
}

impl<W: MidnightWallet> MidnightProvider<W> {
    /// Fill, fund, prove, sign and send the transaction that `request` names.
    ///
    /// This is [`Self::fill`], then [`Self::send_tx_envelope`]. The inputs
    /// that the wallet reserved go back to it in the cases that those two
    /// name.
    ///
    /// ```rust,no_run
    /// # use midnight_provider::{
    /// #     MidnightProvider, ShieldedRecipient, ShieldedTokenType, TransactionRequest,
    /// # };
    /// # async fn f(
    /// #     provider: MidnightProvider<midnight_wallet::Wallet>,
    /// #     token: ShieldedTokenType,
    /// #     bob: ShieldedRecipient,
    /// # ) -> anyhow::Result<()> {
    /// // The receipt fails when the chain does not apply the transfer.
    /// let receipt = provider
    ///     .send_transaction(TransactionRequest::shielded(token, 10, bob))
    ///     .await?
    ///     .get_receipt()
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// The errors of [`Self::fill`] and of [`Self::send_tx_envelope`]. A
    /// request that pays no fee fills, and its send then fails with
    /// [`ProviderError::Transaction`].
    pub async fn send_transaction(
        &self,
        request: TransactionRequest,
    ) -> Result<PendingTx, ProviderError> {
        // Boxed; see the frame-size note on `Self::fund_prove_sign`.
        Box::pin(async move {
            let tx = self.fill_inner(request).await?;
            self.send_tx_envelope(tx).await
        })
        .await
    }

    /// Fill, fund, prove and sign the transaction that `request` names. Send
    /// nothing.
    ///
    /// The provider fills the transaction from node queries. The wallet
    /// resyncs, then funds it and reserves what it spends, as one step, so a
    /// second fill in this process cannot draw the same input. The provider
    /// proves it after that, with the wallet free, and the wallet signs it.
    /// A step that fails hands the inputs back.
    ///
    /// Send the result with [`Self::send_tx_envelope`], or prepare it with
    /// [`Self::prepare`]. A drop of the result hands its inputs back. Its
    /// bytes ([`TxEnvelope::into_bytes`]) keep the inputs reserved until a
    /// sync sees the chain spend them, or until their TTL passes.
    ///
    /// A request made
    /// [`without_fee`](TransactionRequest::without_fee), or a swap half,
    /// gives a transaction that pays no fee. A send refuses it. Hand its bytes
    /// to the payer, who completes it with
    /// [`TransactionRequest::sponsor`], alone or after a
    /// [`merge_transactions`].
    ///
    /// # Errors
    ///
    /// [`ProviderError::Rpc`] when a node query of the fill fails.
    /// [`ProviderError::Wallet`] when the fill refuses the request, or when
    /// the resync, the funding, the proof or the signing fails. The
    /// [`WalletError`](midnight_types::WalletError) names the cause, such as
    /// a shortfall or a TTL that the chain refuses.
    pub async fn fill(&self, request: TransactionRequest) -> Result<TxEnvelope, ProviderError> {
        // Boxed; see the frame-size note on `Self::fund_prove_sign`.
        Box::pin(self.fill_inner(request)).await
    }

    async fn fill_inner(&self, request: TransactionRequest) -> Result<TxEnvelope, ProviderError> {
        let pays_fee = build::pays_fee(&request);
        let filled =
            build::fill(request, self.chain_context(), self.prover.proof_provider()).await?;
        let tx = self.fund_prove_sign(filled).await?;
        Ok(if pays_fee { tx } else { build::fee_less(tx) })
    }

    /// Fund `tx` with the wallet, prove it with this provider's prover, and
    /// let the wallet sign it.
    ///
    /// A step that fails drops the reservation that the funding made, which
    /// hands the inputs back to the wallet.
    async fn fund_prove_sign(&self, tx: FilledTx) -> Result<TxEnvelope, ProviderError> {
        // Boxed so the wallet's resync stays off the caller's frame. A debug
        // build makes this future tens of kilobytes, and an inlined one is
        // carried by every future that awaits it, up to the test or task that
        // owns the stack. Every fill awaits this one.
        Box::pin(async move {
            let funded = self.wallet().fund_transaction(self, tx).await?;
            let proven = build::prove(funded).await?;
            Ok(self.wallet().sign_transaction(proven).await?)
        })
        .await
    }
}

/// Merge proven transactions into one. It reads no node and no wallet.
///
/// Each input is the tagged encoding of a proven transaction, such as the
/// bytes of [`TxEnvelope::into_bytes`]. Merging combines the transactions'
/// intents and Zswap offers and sums their binding randomness. It does not
/// rebalance, so the merge of two swap halves is balanced only when the halves
/// mirror each other.
///
/// **Intent segments must not collide.** The ledger refuses a merge where two
/// inputs carry an intent at the same segment. A contract call and an
/// unshielded transfer put their intent at the fallible segment (1), so at
/// most one input may carry a segment-1 intent. A fee-less shielded transfer
/// and a swap half carry no intent, so they always merge. The supported
/// multi-party shape is "one party pays": the contributors fill
/// [`without_fee`](TransactionRequest::without_fee), and one payer merges
/// their bytes and sends the result with [`TransactionRequest::sponsor`],
/// whose fee intent takes a segment that the merge leaves free.
///
/// ```rust,no_run
/// # use midnight_provider::{
/// #     MidnightProvider, ShieldedTokenType, TransactionRequest, merge_transactions,
/// # };
/// # async fn f(
/// #     alice: MidnightProvider<midnight_wallet::Wallet>,
/// #     bob: MidnightProvider<midnight_wallet::Wallet>,
/// #     token_x: ShieldedTokenType,
/// #     token_y: ShieldedTokenType,
/// # ) -> anyhow::Result<()> {
/// let half_a = alice
///     .fill(TransactionRequest::shielded_swap(token_x, 10, token_y, 20))
///     .await?
///     .into_bytes();
/// let half_b = bob
///     .fill(TransactionRequest::shielded_swap(token_y, 20, token_x, 10))
///     .await?
///     .into_bytes();
/// let swap = merge_transactions(&[half_a, half_b])?;
/// alice
///     .send_transaction(TransactionRequest::sponsor(swap))
///     .await?
///     .get_receipt()
///     .await?;
/// # Ok(())
/// # }
/// ```
///
/// # Errors
///
/// [`ProviderError::Transaction`] when `txs` is empty, when a byte string
/// does not deserialize, or when two transactions cannot be merged
/// (colliding intent segments or different network ids).
pub fn merge_transactions(txs: &[Vec<u8>]) -> Result<Vec<u8>, ProviderError> {
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

fn deserialize_transaction(bytes: &[u8]) -> Result<FinalizedTransaction<DefaultDB>, ProviderError> {
    tagged_deserialize(&mut &bytes[..])
        .map_err(|e| ProviderError::Transaction(format!("deserialize transaction: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Merging nothing is a caller error, not an empty transaction.
    #[test]
    fn merge_transactions_rejects_empty() {
        let err = merge_transactions(&[]).unwrap_err();
        assert!(
            matches!(err, ProviderError::Transaction(ref m) if m.contains("at least one")),
            "got {err:?}"
        );
    }

    /// Undecodable bytes surface as a typed `Transaction` error naming the
    /// failing step, not a panic inside the ledger deserializer.
    #[test]
    fn merge_transactions_rejects_invalid_bytes() {
        let err = merge_transactions(&[vec![0xFF; 8]]).unwrap_err();
        assert!(
            matches!(err, ProviderError::Transaction(ref m) if m.contains("deserialize")),
            "got {err:?}"
        );
    }
}
