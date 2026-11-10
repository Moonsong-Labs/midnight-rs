//! Pending transfer / dust-registration builders.
//!
//! Each builder represents a wallet operation that has been *requested* but not
//! yet executed. Call sites have two endpoints:
//!
//! - `.send().await?`: the one-shot path. Builds (which reserves pending UTXOs /
//!   dust so concurrent in-process builds don't double-select), submits the
//!   proven transaction bytes to the node, and returns a [`PendingTx`] so the
//!   caller chooses how to wait (`wait_best` / `wait_finalized`).
//! - `.build().await?`: the escape hatch. Returns the signed [`TxEnvelope`]
//!   without submitting. The inputs stay reserved. Submit the envelope with
//!   [`MidnightProvider::submit_reserved`], which hands them back when the
//!   node never gets the transaction or rejects it. A drop of the envelope
//!   hands them back too. Its bytes ([`TxEnvelope::into_bytes`]) keep them
//!   until a sync sees the chain spend them, or until their TTL passes.
//!
//! Constructors are sync methods on [`MidnightProvider`]: `transfer_unshielded`,
//! `transfer_shielded`, `register_dust`. They borrow the provider and capture
//! the operation's inputs; no work happens until the caller calls `.send()`
//! or `.build()`.
//!
//! ```rust,no_run
//! # use midnight_provider::{
//! #     MidnightProvider, NIGHT, ShieldedRecipient, ShieldedTokenType, UnshieldedRecipient,
//! # };
//! # async fn f(
//! #     provider: MidnightProvider<midnight_wallet::Wallet>,
//! #     unshielded: UnshieldedRecipient,
//! #     shielded: ShieldedRecipient,
//! #     token: ShieldedTokenType,
//! # ) -> anyhow::Result<()> {
//! // One-shot: build and submit, then wait however you like. The wait fails
//! // when the chain does not apply the transfer.
//! let pending = provider
//!     .transfer_unshielded(NIGHT, 100, unshielded)
//!     .send()
//!     .await?;
//! pending.wait_finalized().await?;
//!
//! // Build only, then submit with the reservation.
//! let envelope = provider.transfer_shielded(token, 1, shielded).build().await?;
//! provider.submit_reserved(envelope).await?.wait_finalized().await?;
//! # Ok(())
//! # }
//! ```

use std::future::{Future, IntoFuture};
use std::pin::Pin;

use midnight_types::build::{TransferKind, TransferRequest};
use midnight_types::{
    CoinSelectionStrategy, DustPublicKey, MidnightWallet, ShieldedRecipient, ShieldedTokenType,
    TxEnvelope, UnshieldedRecipient, UnshieldedTokenType,
};

use crate::{MidnightProvider, PendingTx, ProviderError};

/// Pending unshielded transfer. See [`MidnightProvider::transfer_shielded`]
/// for the `.send()` vs `.build()` distinction.
#[derive(Debug)]
#[must_use = "does nothing until sent or built"]
pub struct UnshieldedTransfer<'a, W> {
    provider: &'a MidnightProvider<W>,
    token_type: UnshieldedTokenType,
    amount: u128,
    recipient: UnshieldedRecipient,
    coin_selection: CoinSelectionStrategy,
    ttl: Option<std::time::Duration>,
}

impl<'a, W: MidnightWallet> UnshieldedTransfer<'a, W> {
    pub(crate) fn new(
        provider: &'a MidnightProvider<W>,
        token_type: UnshieldedTokenType,
        amount: u128,
        recipient: UnshieldedRecipient,
    ) -> Self {
        Self {
            provider,
            token_type,
            amount,
            recipient,
            coin_selection: CoinSelectionStrategy::default(),
            ttl: None,
        }
    }

    /// Order the coins and UTXOs that this build draws on.
    ///
    /// [`CoinSelectionStrategy::LargestFirst`], the default, spends the fewest
    /// inputs, and each shielded input adds a proof and a fee.
    /// [`CoinSelectionStrategy::SmallestFirst`] consolidates small coins that
    /// the default never spends. Nothing caps how many inputs it draws, so use
    /// it for a deliberate consolidation, not for an ordinary payment.
    pub fn with_coin_selection(mut self, strategy: CoinSelectionStrategy) -> Self {
        self.coin_selection = strategy;
        self
    }

    /// Make the transaction expire `ttl` after the time of the node's best
    /// block at the build. Defaults to the chain's `global_ttl`, the longest
    /// TTL the chain accepts. The build fails with
    /// [`WalletError::TtlOutOfRange`](midnight_types::WalletError::TtlOutOfRange)
    /// when `ttl` is under one second or longer than `global_ttl`.
    ///
    /// Once the TTL passes, the transaction can no longer land, and the next
    /// build frees the inputs it reserved. So after a submit whose fate is
    /// unknown, such as [`SubmitError::Dropped`](crate::SubmitError::Dropped),
    /// a rebuild from the same inputs is safe from a double spend after
    /// `ttl`, not after the whole `global_ttl`.
    pub fn with_ttl(mut self, ttl: std::time::Duration) -> Self {
        self.ttl = Some(ttl);
        self
    }

    /// Build the transaction, submit it with
    /// [`MidnightProvider::submit_reserved`], and return the [`PendingTx`] to
    /// wait on.
    pub async fn send(self) -> Result<PendingTx, ProviderError> {
        let provider = self.provider;
        // Boxed; see the frame-size note on `MidnightProvider::fund_prove_sign`.
        Box::pin(build_and_submit(provider, self.into_request(true))).await
    }

    /// Build the transaction without submitting it. Reserves pending UTXOs /
    /// dust in the wallet so concurrent in-process builds don't double-select
    /// the same inputs.
    pub async fn build(self) -> Result<TxEnvelope, ProviderError> {
        let provider = self.provider;
        provider.build_transfer(self.into_request(true)).await
    }

    fn into_request(self, pay_fees: bool) -> TransferRequest {
        let mut request = TransferRequest::new(TransferKind::Unshielded {
            token_type: self.token_type,
            amount: self.amount,
            recipient: self.recipient,
            pay_fees,
        })
        .with_coin_selection(self.coin_selection);
        request.ttl = self.ttl;
        request
    }
}

/// Pending shielded transfer. See [`MidnightProvider::transfer_shielded`] for
/// the `.send()` vs `.build()` distinction.
#[derive(Debug)]
#[must_use = "does nothing until sent or built"]
pub struct ShieldedTransfer<'a, W> {
    provider: &'a MidnightProvider<W>,
    token_type: ShieldedTokenType,
    amount: u128,
    recipient: ShieldedRecipient,
    coin_selection: CoinSelectionStrategy,
    ttl: Option<std::time::Duration>,
}

impl<'a, W: MidnightWallet> ShieldedTransfer<'a, W> {
    pub(crate) fn new(
        provider: &'a MidnightProvider<W>,
        token_type: ShieldedTokenType,
        amount: u128,
        recipient: ShieldedRecipient,
    ) -> Self {
        Self {
            provider,
            token_type,
            amount,
            recipient,
            coin_selection: CoinSelectionStrategy::default(),
            ttl: None,
        }
    }

    /// Order the coins that this build draws on. See
    /// [`UnshieldedTransfer::with_coin_selection`].
    pub fn with_coin_selection(mut self, strategy: CoinSelectionStrategy) -> Self {
        self.coin_selection = strategy;
        self
    }

    /// Make the transaction expire `ttl` after the chain time of the build.
    /// See [`UnshieldedTransfer::with_ttl`].
    ///
    /// A Dustless build, [`DustlessBuilder::without_dust`], refuses a TTL with
    /// [`WalletError::TtlWithoutIntent`](midnight_types::WalletError::TtlWithoutIntent):
    /// a shielded transfer that pays no fee is a Zswap offer alone, with no
    /// intent to carry it.
    pub fn with_ttl(mut self, ttl: std::time::Duration) -> Self {
        self.ttl = Some(ttl);
        self
    }

    /// Build the transaction, submit it with
    /// [`MidnightProvider::submit_reserved`], and return the [`PendingTx`] to
    /// wait on.
    pub async fn send(self) -> Result<PendingTx, ProviderError> {
        let provider = self.provider;
        // Boxed; see the frame-size note on `MidnightProvider::fund_prove_sign`.
        Box::pin(build_and_submit(provider, self.into_request(true))).await
    }

    /// Build the transaction without submitting it. Reserves pending dust in
    /// the wallet so concurrent in-process builds don't double-select.
    pub async fn build(self) -> Result<TxEnvelope, ProviderError> {
        let provider = self.provider;
        provider.build_transfer(self.into_request(true)).await
    }

    fn into_request(self, pay_fees: bool) -> TransferRequest {
        let mut request = TransferRequest::new(TransferKind::Shielded {
            token_type: self.token_type,
            amount: self.amount,
            recipient: self.recipient,
            pay_fees,
        })
        .with_coin_selection(self.coin_selection);
        request.ttl = self.ttl;
        request
    }
}

/// Pending shielded swap half. The transfer builders submit with `.send()`.
/// Awaiting this yields a [`DustlessTransaction`] and submits nothing: a swap
/// half is inherently unbalanced and fee-less, so it is never submittable on
/// its own (there is no `pay_fees` path and no `.without_dust()` step). See
/// [`MidnightProvider::shielded_swap`] for the full two-party flow.
#[derive(Debug)]
#[must_use = "does nothing until awaited or built"]
pub struct ShieldedSwap<'a, W> {
    provider: &'a MidnightProvider<W>,
    give_token: ShieldedTokenType,
    give_amount: u128,
    receive_token: ShieldedTokenType,
    receive_amount: u128,
    coin_selection: CoinSelectionStrategy,
}

impl<'a, W: MidnightWallet> ShieldedSwap<'a, W> {
    pub(crate) fn new(
        provider: &'a MidnightProvider<W>,
        give_token: ShieldedTokenType,
        give_amount: u128,
        receive_token: ShieldedTokenType,
        receive_amount: u128,
    ) -> Self {
        Self {
            provider,
            give_token,
            give_amount,
            receive_token,
            receive_amount,
            coin_selection: CoinSelectionStrategy::default(),
        }
    }

    /// Order the coins that this build draws on. See
    /// [`UnshieldedTransfer::with_coin_selection`].
    pub fn with_coin_selection(mut self, strategy: CoinSelectionStrategy) -> Self {
        self.coin_selection = strategy;
        self
    }

    /// Build the swap half without wrapping it, returning the signed
    /// [`TxEnvelope`]. Reserves the spent give-side coins so concurrent
    /// in-process builds don't re-select them, and a drop of the envelope
    /// hands them back. Most callers want the [`DustlessTransaction`] from
    /// `.await`.
    pub async fn build(self) -> Result<TxEnvelope, ProviderError> {
        let request = TransferRequest::new(TransferKind::ShieldedSwap {
            give_token: self.give_token,
            give_amount: self.give_amount,
            receive_token: self.receive_token,
            receive_amount: self.receive_amount,
        })
        .with_coin_selection(self.coin_selection);
        self.provider.build_transfer(request).await
    }
}

impl<'a, W: MidnightWallet> IntoFuture for ShieldedSwap<'a, W> {
    type Output = Result<DustlessTransaction, ProviderError>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send + 'a>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let envelope = self.build().await?;
            Ok(DustlessTransaction::from_proven_bytes(
                envelope.into_bytes(),
            ))
        })
    }
}

/// A transaction builder that can drop its Dust (fees), so another wallet can
/// sponsor them. `.without_dust()` is uniform across builder kinds because
/// paying fees is a general transaction concern, not tied to any transaction
/// kind. Implemented for shielded transfers, unshielded transfers, and
/// generated contract-call builders.
///
/// Bring this trait into scope to call [`without_dust`](Self::without_dust).
pub trait DustlessBuilder: Sized {
    /// The builder crate's error type.
    type Error;

    /// Build and prove this transaction **without paying its Dust fees**,
    /// reserving any spent inputs so concurrent in-process builds don't
    /// double-select them. Returns a [`DustlessTransaction`], which is not
    /// submittable on its own: hand it to the fee payer, who completes it with
    /// [`MidnightProvider::balance_transaction`] (one payer) or
    /// [`MidnightProvider::merge_transactions`], then submits.
    fn without_dust(self) -> impl Future<Output = Result<DustlessTransaction, Self::Error>> + Send;
}

impl<'a, W: MidnightWallet> DustlessBuilder for ShieldedTransfer<'a, W> {
    type Error = ProviderError;
    async fn without_dust(self) -> Result<DustlessTransaction, ProviderError> {
        let provider = self.provider;
        let envelope = provider.build_transfer(self.into_request(false)).await?;
        Ok(DustlessTransaction::from_proven_bytes(
            envelope.into_bytes(),
        ))
    }
}

impl<'a, W: MidnightWallet> DustlessBuilder for UnshieldedTransfer<'a, W> {
    type Error = ProviderError;
    async fn without_dust(self) -> Result<DustlessTransaction, ProviderError> {
        let provider = self.provider;
        let envelope = provider.build_transfer(self.into_request(false)).await?;
        Ok(DustlessTransaction::from_proven_bytes(
            envelope.into_bytes(),
        ))
    }
}

/// A proven transaction that carries its effects but pays **no Dust** (no
/// fees). Produced by `.without_dust()` on any builder.
///
/// Dust is Midnight's fee token, so "Dustless" names the general fee-less state
/// of a transaction regardless of what it does. It is not submittable on its
/// own: complete it with [`MidnightProvider::balance_transaction`] (one wallet
/// sponsors the fees) or fold it into a larger transaction with
/// [`MidnightProvider::merge_transactions`], then submit.
pub struct DustlessTransaction {
    tx_bytes: Vec<u8>,
}

impl std::fmt::Debug for DustlessTransaction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DustlessTransaction")
            .field("tx_bytes", &format_args!("{} bytes", self.tx_bytes.len()))
            .finish()
    }
}

impl DustlessTransaction {
    /// Wrap already-proven, Dustless transaction bytes. Called by the
    /// `.without_dust()` build paths (transfers here, generated contract-call
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

/// Pending dust-address registration. See
/// [`MidnightProvider::transfer_shielded`] for the `.send()` vs `.build()`
/// distinction.
#[derive(Debug)]
#[must_use = "does nothing until sent or built"]
pub struct DustRegistration<'a, W> {
    provider: &'a MidnightProvider<W>,
    utxo_ctime: Option<u64>,
    target: Option<DustPublicKey>,
    ttl: Option<std::time::Duration>,
}

impl<'a, W: MidnightWallet> DustRegistration<'a, W> {
    pub(crate) fn new(provider: &'a MidnightProvider<W>, utxo_ctime: Option<u64>) -> Self {
        Self {
            provider,
            utxo_ctime,
            target: None,
            ttl: None,
        }
    }

    /// Send the Dust of the registered tNIGHT to `target`, another wallet's
    /// Dust public key. That wallet can then pay the fees of this wallet's
    /// transactions with [`MidnightProvider::balance_transaction`]. Read
    /// `target` from the Dust address of that wallet with
    /// [`parse_dust_address`](midnight_types::address::parse_dust_address).
    ///
    /// The registration sets the Dust public key of the whole NIGHT address.
    /// A later registration replaces it, so a registration without `.to()`
    /// points the NIGHT address back to this wallet's own key. tNIGHT that
    /// reaches the address generates Dust for the key that the last
    /// registration set. tNIGHT that already generates Dust keeps its key
    /// until the wallet spends it.
    pub fn to(mut self, target: DustPublicKey) -> Self {
        self.target = Some(target);
        self
    }

    /// Make the transaction expire `ttl` after the chain time of the build.
    /// See [`UnshieldedTransfer::with_ttl`].
    pub fn with_ttl(mut self, ttl: std::time::Duration) -> Self {
        self.ttl = Some(ttl);
        self
    }

    /// Build the transaction, submit it with
    /// [`MidnightProvider::submit_reserved`], and return the [`PendingTx`] to
    /// wait on.
    pub async fn send(self) -> Result<PendingTx, ProviderError> {
        let provider = self.provider;
        // Boxed; see the frame-size note on `MidnightProvider::fund_prove_sign`.
        Box::pin(build_and_submit(provider, self.into_request())).await
    }

    /// Build the registration transaction without submitting. Spends and
    /// re-creates one of the wallet's tNIGHT UTXOs as part of the build.
    pub async fn build(self) -> Result<TxEnvelope, ProviderError> {
        let provider = self.provider;
        provider.build_transfer(self.into_request()).await
    }

    fn into_request(self) -> TransferRequest {
        let mut request = TransferRequest::from(TransferKind::DustRegistration {
            utxo_ctime: self.utxo_ctime,
            target: self.target,
        });
        request.ttl = self.ttl;
        request
    }
}

async fn build_and_submit<W: MidnightWallet>(
    provider: &MidnightProvider<W>,
    request: TransferRequest,
) -> Result<PendingTx, ProviderError> {
    let envelope = provider.build_transfer(request).await?;
    provider.submit_reserved(envelope).await
}
