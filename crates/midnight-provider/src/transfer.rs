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
//!   sign it elsewhere, route submission through something other than
//!   `provider.submit(...)`, or read [`TransferResult::fee_speck`] to show
//!   the user the deterministic Dust fee before they confirm.
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

    /// Build this transfer **without paying its Dust fees**, for a multi-party
    /// flow where another wallet sponsors them. Returns a [`WithoutFees`]
    /// build-only handle (no submit path — a Dustless transaction is not valid
    /// on its own). See [`WithoutFees::build`].
    pub fn without_fees(self) -> WithoutFees<Self> {
        WithoutFees(self)
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

/// A builder wrapped to skip Dust (fee) funding, produced by `.without_fees()`.
///
/// Generic over the underlying builder `B` because paying fees is a general
/// transaction concern, not tied to any transaction kind: `WithoutFees<`
/// [`ShieldedTransfer`]`>` today, `WithoutFees<`[`UnshieldedTransfer`]`>` and a
/// contract call's `WithoutFees<..>` the same way. Deliberately exposes only
/// `build` (no [`IntoFuture`] submit path): a [`DustlessTransaction`] is not
/// valid on its own, so it can't be submitted directly.
pub struct WithoutFees<B>(B);

impl<B> WithoutFees<B> {
    /// Wrap a builder to skip Dust funding. Used by `.without_fees()` on each
    /// builder (including generated contract-call builders in other crates).
    pub fn new(builder: B) -> Self {
        WithoutFees(builder)
    }

    /// The wrapped builder.
    pub fn into_inner(self) -> B {
        self.0
    }
}

impl<B: DustlessBuilder> WithoutFees<B> {
    /// Build the proven, token-balanced but **Dustless** transaction (reserving
    /// any spent inputs so concurrent in-process builds don't double-select
    /// them). Hand the result to the fee payer, who completes it with
    /// [`MidnightProvider::balance_transaction`] (one payer) or
    /// [`MidnightProvider::merge_transactions`].
    ///
    /// Inherent (no trait import needed at the call site); the per-builder work
    /// lives in each builder's [`DustlessBuilder`] impl.
    pub async fn build(self) -> Result<DustlessTransaction, B::Error> {
        self.into_inner().build_dustless().await
    }
}

/// A builder that can produce a [`DustlessTransaction`] (a proven, fee-less
/// transaction). Implemented by every `.without_fees()`-capable builder,
/// transfers here, generated contract-call builders in other crates, so
/// [`WithoutFees::build`] is uniform. Paying fees is a general transaction
/// concern, so this is not tied to any transaction kind. Not called directly:
/// use `.without_fees().build()`.
pub trait DustlessBuilder {
    /// The builder crate's error type.
    type Error;
    /// Build and prove the Dustless transaction.
    fn build_dustless(
        self,
    ) -> impl Future<Output = Result<DustlessTransaction, Self::Error>> + Send;
}

impl<'a> DustlessBuilder for ShieldedTransfer<'a> {
    type Error = ProviderError;
    async fn build_dustless(self) -> Result<DustlessTransaction, ProviderError> {
        let result = self
            .provider
            .build_shielded_transfer_without_fees(self.token_type, self.amount, &self.recipient)
            .await?;
        Ok(DustlessTransaction::from_proven_bytes(result.tx_bytes))
    }
}

/// A proven transaction that carries its effects but pays **no Dust** (no
/// fees). Produced by `.without_fees().build()` on any builder.
///
/// Dust is Midnight's fee token, so "Dustless" names the general fee-less state
/// of a transaction regardless of what it does. It is not submittable on its
/// own: complete it with [`MidnightProvider::balance_transaction`] (one wallet
/// sponsors the fees) or fold it into a larger transaction with
/// [`MidnightProvider::merge_transactions`], then submit.
pub struct DustlessTransaction {
    tx_bytes: Vec<u8>,
}

impl DustlessTransaction {
    /// Wrap already-proven, Dustless transaction bytes. Called by the
    /// `.without_fees()` build paths (transfers here, generated contract-call
    /// builders in other crates); not intended for wrapping arbitrary bytes.
    pub fn from_proven_bytes(tx_bytes: Vec<u8>) -> Self {
        DustlessTransaction { tx_bytes }
    }

    /// The proven transaction bytes, to hand to the fee payer.
    pub fn as_bytes(&self) -> &[u8] {
        &self.tx_bytes
    }

    /// Consume this, returning the proven transaction bytes.
    pub fn into_bytes(self) -> Vec<u8> {
        self.tx_bytes
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
