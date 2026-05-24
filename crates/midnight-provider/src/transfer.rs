//! Pending transfer / dust-registration builders.
//!
//! Each builder represents a wallet operation that has been *requested* but not
//! yet executed. Call sites have two endpoints:
//!
//! - `.await?` — the one-shot path. Builds (which reserves pending UTXOs / dust
//!   so concurrent in-process builds don't double-select), submits the proven
//!   transaction bytes to the node, and returns a [`PendingTx`] so the caller
//!   chooses how to wait (`wait_best` / `wait_finalized`).
//! - `.build().await?` — the escape hatch. Returns the [`TransferResult`]
//!   without submitting. Useful when the caller wants to inspect `tx_bytes`,
//!   sign it elsewhere, or route submission through something other than
//!   `provider.submit(...)`.
//!
//! Constructors are sync methods on [`MidnightProvider`]: `transfer_unshielded`,
//! `transfer_shielded`, `register_dust`. They borrow the provider and capture
//! the operation's inputs; no work happens until the caller awaits or calls
//! `.build()`.
//!
//! ```rust,ignore
//! // One-shot — build + submit, then wait however you like:
//! let pending = provider.transfer_unshielded(NIGHT, 100, &recipient).await?;
//! let (_, _) = pending.wait_best().await?;
//!
//! // Build only — keep tx_bytes around for custom routing:
//! let result = provider.transfer_shielded(token, 1, &recipient).build().await?;
//! ```

use std::future::{Future, IntoFuture};
use std::pin::Pin;

use midnight_wallet::{ShieldedTokenType, TransferResult, UnshieldedTokenType};

use crate::{MidnightProvider, PendingTx, ProviderError};

/// Pending unshielded transfer. See [module docs](crate::transfer) for the
/// `.await` vs `.build()` distinction.
pub struct UnshieldedTransfer<'a> {
    provider: &'a MidnightProvider,
    token_type: UnshieldedTokenType,
    amount: u128,
    recipient: String,
}

impl<'a> UnshieldedTransfer<'a> {
    pub(crate) fn new(
        provider: &'a MidnightProvider,
        token_type: UnshieldedTokenType,
        amount: u128,
        recipient: &str,
    ) -> Self {
        Self {
            provider,
            token_type,
            amount,
            recipient: recipient.to_string(),
        }
    }

    /// Build the transaction without submitting it. Reserves pending UTXOs /
    /// dust in the wallet so concurrent in-process builds don't double-select
    /// the same inputs.
    pub async fn build(self) -> Result<TransferResult, ProviderError> {
        self.provider
            .build_unshielded_transfer(self.token_type, self.amount, &self.recipient)
            .await
    }
}

impl<'a> IntoFuture for UnshieldedTransfer<'a> {
    type Output = Result<PendingTx, ProviderError>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send + 'a>>;

    fn into_future(self) -> Self::IntoFuture {
        let provider = self.provider;
        Box::pin(async move {
            let result = self.build().await?;
            provider.submit(&result.tx_bytes).await
        })
    }
}

/// Pending shielded transfer. See [module docs](crate::transfer) for the
/// `.await` vs `.build()` distinction.
pub struct ShieldedTransfer<'a> {
    provider: &'a MidnightProvider,
    token_type: ShieldedTokenType,
    amount: u128,
    recipient: String,
}

impl<'a> ShieldedTransfer<'a> {
    pub(crate) fn new(
        provider: &'a MidnightProvider,
        token_type: ShieldedTokenType,
        amount: u128,
        recipient: &str,
    ) -> Self {
        Self {
            provider,
            token_type,
            amount,
            recipient: recipient.to_string(),
        }
    }

    /// Build the transaction without submitting it. Reserves pending dust in
    /// the wallet so concurrent in-process builds don't double-select.
    pub async fn build(self) -> Result<TransferResult, ProviderError> {
        self.provider
            .build_shielded_transfer(self.token_type, self.amount, &self.recipient)
            .await
    }
}

impl<'a> IntoFuture for ShieldedTransfer<'a> {
    type Output = Result<PendingTx, ProviderError>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send + 'a>>;

    fn into_future(self) -> Self::IntoFuture {
        let provider = self.provider;
        Box::pin(async move {
            let result = self.build().await?;
            provider.submit(&result.tx_bytes).await
        })
    }
}

/// Pending dust-address registration. See [module docs](crate::transfer) for
/// the `.await` vs `.build()` distinction.
pub struct DustRegistration<'a> {
    provider: &'a MidnightProvider,
    utxo_ctime: Option<u64>,
}

impl<'a> DustRegistration<'a> {
    pub(crate) fn new(provider: &'a MidnightProvider, utxo_ctime: Option<u64>) -> Self {
        Self {
            provider,
            utxo_ctime,
        }
    }

    /// Build the registration transaction without submitting. Spends and
    /// re-creates the wallet's tNIGHT UTXOs as part of the build.
    pub async fn build(self) -> Result<TransferResult, ProviderError> {
        self.provider.build_register_dust(self.utxo_ctime).await
    }
}

impl<'a> IntoFuture for DustRegistration<'a> {
    type Output = Result<PendingTx, ProviderError>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send + 'a>>;

    fn into_future(self) -> Self::IntoFuture {
        let provider = self.provider;
        Box::pin(async move {
            let result = self.build().await?;
            provider.submit(&result.tx_bytes).await
        })
    }
}
