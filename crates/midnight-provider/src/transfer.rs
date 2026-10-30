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
//!   or read [`TransferResult::fee_speck`] to show the user the
//!   deterministic Dust fee before they confirm. The inputs stay reserved.
//!   Submit the result with
//!   [`MidnightProvider::submit_reserved`], which hands them back when the
//!   node never gets the transaction or rejects it. Bytes that never reach
//!   the chain keep them until [`MidnightProvider::release`] gets
//!   `result.spent`, or until their TTL passes. A fee-less build keeps its
//!   reservation on the [`DustlessTransaction`] (see
//!   [`DustlessTransaction::spent`]).
//!
//! Constructors are sync methods on [`MidnightProvider`]: `transfer_unshielded`,
//! `transfer_shielded`, `register_dust`. They borrow the provider and capture
//! the operation's inputs; no work happens until the caller awaits or calls
//! `.build()`.
//!
//! ```rust,no_run
//! # use midnight_provider::{MidnightProvider, NIGHT, ShieldedTokenType};
//! # async fn f(
//! #     provider: MidnightProvider,
//! #     recipient: String,
//! #     token: ShieldedTokenType,
//! # ) -> anyhow::Result<()> {
//! // One-shot: build and submit, then wait however you like. The wait fails
//! // when the chain does not apply the transfer.
//! let pending = provider.transfer_unshielded(NIGHT, 100, &recipient).await?;
//! pending.wait_finalized().await?;
//!
//! // Build only: read the fee, then submit with the reservation.
//! let result = provider.transfer_shielded(token, 1, &recipient).build().await?;
//! println!("fee: {} SPECK", result.fee_speck);
//! provider.submit_reserved(result).await?.wait_finalized().await?;
//! # Ok(())
//! # }
//! ```

use std::future::{Future, IntoFuture};
use std::pin::Pin;

use midnight_types::{
    DustPublicKey, ShieldedTokenType, SpentInputs, TransferKind, TransferRequest, TransferResult,
    UnshieldedTokenType,
};

use crate::{MidnightProvider, PendingTx, ProviderError};

/// Pending unshielded transfer. See [module docs](crate::transfer) for the
/// `.await` vs `.build()` distinction.
#[derive(Debug)]
#[must_use = "does nothing until awaited or built"]
pub struct UnshieldedTransfer<'a> {
    provider: &'a MidnightProvider,
    token_type: UnshieldedTokenType,
    amount: u128,
    recipient: String,
    coin_selection: midnight_types::CoinSelectionStrategy,
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
            coin_selection: midnight_types::CoinSelectionStrategy::default(),
        }
    }

    /// Order the coins and UTXOs this build draws on. See
    /// [`TransferBuilder::with_coin_selection`](midnight_types::ledger_9::TransferBuilder::with_coin_selection).
    /// Defaults to [`CoinSelectionStrategy::LargestFirst`](midnight_types::CoinSelectionStrategy::LargestFirst),
    /// which spends the fewest inputs.
    pub fn with_coin_selection(mut self, strategy: midnight_types::CoinSelectionStrategy) -> Self {
        self.coin_selection = strategy;
        self
    }

    /// Build the transaction without submitting it. Reserves pending UTXOs /
    /// dust in the wallet so concurrent in-process builds don't double-select
    /// the same inputs.
    pub async fn build(self) -> Result<TransferResult, ProviderError> {
        let provider = self.provider;
        provider.build_then_prove(self.into_request(true)).await
    }

    fn into_request(self, pay_fees: bool) -> TransferRequest {
        TransferRequest::new(TransferKind::Unshielded {
            token_type: self.token_type,
            amount: self.amount,
            recipient: self.recipient,
            pay_fees,
        })
        .with_coin_selection(self.coin_selection)
    }
}

impl<'a> IntoFuture for UnshieldedTransfer<'a> {
    type Output = Result<PendingTx, ProviderError>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send + 'a>>;

    fn into_future(self) -> Self::IntoFuture {
        let provider = self.provider;
        Box::pin(build_and_submit(provider, self.into_request(true)))
    }
}

/// Pending shielded transfer. See [module docs](crate::transfer) for the
/// `.await` vs `.build()` distinction.
#[derive(Debug)]
#[must_use = "does nothing until awaited or built"]
pub struct ShieldedTransfer<'a> {
    provider: &'a MidnightProvider,
    token_type: ShieldedTokenType,
    amount: u128,
    recipient: String,
    coin_selection: midnight_types::CoinSelectionStrategy,
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
            coin_selection: midnight_types::CoinSelectionStrategy::default(),
        }
    }

    /// Order the coins and UTXOs this build draws on. See
    /// [`TransferBuilder::with_coin_selection`](midnight_types::ledger_9::TransferBuilder::with_coin_selection).
    /// Defaults to [`CoinSelectionStrategy::LargestFirst`](midnight_types::CoinSelectionStrategy::LargestFirst),
    /// which spends the fewest inputs.
    pub fn with_coin_selection(mut self, strategy: midnight_types::CoinSelectionStrategy) -> Self {
        self.coin_selection = strategy;
        self
    }

    /// Build the transaction without submitting it. Reserves pending dust in
    /// the wallet so concurrent in-process builds don't double-select.
    pub async fn build(self) -> Result<TransferResult, ProviderError> {
        let provider = self.provider;
        provider.build_then_prove(self.into_request(true)).await
    }

    fn into_request(self, pay_fees: bool) -> TransferRequest {
        TransferRequest::new(TransferKind::Shielded {
            token_type: self.token_type,
            amount: self.amount,
            recipient: self.recipient,
            pay_fees,
        })
        .with_coin_selection(self.coin_selection)
    }
}

impl<'a> IntoFuture for ShieldedTransfer<'a> {
    type Output = Result<PendingTx, ProviderError>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send + 'a>>;

    fn into_future(self) -> Self::IntoFuture {
        let provider = self.provider;
        Box::pin(build_and_submit(provider, self.into_request(true)))
    }
}

/// Pending shielded swap half. Unlike the transfer builders, awaiting this
/// yields a [`DustlessTransaction`] directly rather than submitting: a swap
/// half is inherently unbalanced and fee-less, so it is never submittable on
/// its own (there is no `pay_fees` path and no `.without_dust()` step). See
/// [`MidnightProvider::shielded_swap`] for the full two-party flow.
#[derive(Debug)]
#[must_use = "does nothing until awaited or built"]
pub struct ShieldedSwap<'a> {
    provider: &'a MidnightProvider,
    give_token: ShieldedTokenType,
    give_amount: u128,
    receive_token: ShieldedTokenType,
    receive_amount: u128,
    coin_selection: midnight_types::CoinSelectionStrategy,
}

impl<'a> ShieldedSwap<'a> {
    pub(crate) fn new(
        provider: &'a MidnightProvider,
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
            coin_selection: midnight_types::CoinSelectionStrategy::default(),
        }
    }

    /// Order the coins and UTXOs this build draws on. See
    /// [`TransferBuilder::with_coin_selection`](midnight_types::ledger_9::TransferBuilder::with_coin_selection).
    /// Defaults to [`CoinSelectionStrategy::LargestFirst`](midnight_types::CoinSelectionStrategy::LargestFirst),
    /// which spends the fewest inputs.
    pub fn with_coin_selection(mut self, strategy: midnight_types::CoinSelectionStrategy) -> Self {
        self.coin_selection = strategy;
        self
    }

    /// Build the swap half without wrapping it, returning the raw
    /// [`TransferResult`]. Reserves the spent give-side coins so concurrent
    /// in-process builds don't re-select them. Most callers want the
    /// [`DustlessTransaction`] from `.await`; use this only to inspect the raw
    /// `tx_bytes`. Note [`TransferResult::fee_speck`] here prices this half in
    /// isolation and is not the fee the merged swap pays; the sponsor learns
    /// that when [`MidnightProvider::balance_transaction`] funds the merge.
    pub async fn build(self) -> Result<TransferResult, ProviderError> {
        let request = TransferRequest::new(TransferKind::ShieldedSwap {
            give_token: self.give_token,
            give_amount: self.give_amount,
            receive_token: self.receive_token,
            receive_amount: self.receive_amount,
        })
        .with_coin_selection(self.coin_selection);
        self.provider.build_then_prove(request).await
    }
}

impl<'a> IntoFuture for ShieldedSwap<'a> {
    type Output = Result<DustlessTransaction, ProviderError>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send + 'a>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move { self.build().await.map(DustlessTransaction::reserved) })
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
    ///
    /// The [`DustlessTransaction::spent`] of a transfer names the inputs that
    /// the build reserved, for a release when the fee payer never submits.
    /// That of a generated contract call is empty.
    fn without_dust(self) -> impl Future<Output = Result<DustlessTransaction, Self::Error>> + Send;
}

impl<'a> DustlessBuilder for ShieldedTransfer<'a> {
    type Error = ProviderError;
    async fn without_dust(self) -> Result<DustlessTransaction, ProviderError> {
        let provider = self.provider;
        provider
            .build_then_prove(self.into_request(false))
            .await
            .map(DustlessTransaction::reserved)
    }
}

impl<'a> DustlessBuilder for UnshieldedTransfer<'a> {
    type Error = ProviderError;
    async fn without_dust(self) -> Result<DustlessTransaction, ProviderError> {
        let provider = self.provider;
        provider
            .build_then_prove(self.into_request(false))
            .await
            .map(DustlessTransaction::reserved)
    }
}

/// A proven transaction that carries its effects but pays **no Dust** (no
/// fees). Produced by `.without_dust()` on any builder, and by awaiting a
/// [`ShieldedSwap`].
///
/// Dust is Midnight's fee token, so "Dustless" names the general fee-less state
/// of a transaction regardless of what it does. It is not submittable on its
/// own: complete it with [`MidnightProvider::balance_transaction`] (one wallet
/// sponsors the fees) or fold it into a larger transaction with
/// [`MidnightProvider::merge_transactions`], then submit.
///
/// It also names the inputs that its build reserved, in [`Self::spent`].
pub struct DustlessTransaction {
    tx_bytes: Vec<u8>,
    spent: SpentInputs,
}

impl std::fmt::Debug for DustlessTransaction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DustlessTransaction")
            .field("tx_bytes", &format_args!("{} bytes", self.tx_bytes.len()))
            .field("spent", &self.spent)
            .finish()
    }
}

impl DustlessTransaction {
    /// Wrap already-proven, Dustless transaction bytes that do not name their
    /// reserved inputs. Do not use it to wrap arbitrary bytes.
    ///
    /// [`Self::spent`] of the result is empty.
    pub fn from_proven_bytes(tx_bytes: Vec<u8>) -> Self {
        DustlessTransaction {
            tx_bytes,
            spent: SpentInputs::default(),
        }
    }

    fn reserved(result: TransferResult) -> Self {
        DustlessTransaction {
            tx_bytes: result.tx_bytes,
            spent: result.spent,
        }
    }

    /// The proven transaction bytes, to hand to the fee payer.
    pub fn as_bytes(&self) -> &[u8] {
        &self.tx_bytes
    }

    /// Consume this, returning the proven transaction bytes. Use
    /// [`Self::into_parts`] to keep [`Self::spent`] too.
    pub fn into_bytes(self) -> Vec<u8> {
        self.tx_bytes
    }

    /// The inputs that the build of this transaction reserved in the wallet
    /// of the provider that built it.
    ///
    /// They stay reserved while the fee payer completes and submits the
    /// transaction, so a later build of that wallet does not select them.
    /// When the transaction will not land, pass them to
    /// [`MidnightProvider::release`] on that provider. For example, the
    /// counterparty or the fee payer gives up. Otherwise they stay reserved
    /// until a sync sees the chain spend them, or until their TTL passes. A
    /// fee payer that submits with [`MidnightProvider::submit_reserved`]
    /// hands back only its own Dust when the node rejects the transaction.
    /// So these inputs need this release too.
    ///
    /// Release only a transaction that cannot land. A shielded transfer or a
    /// swap half has no TTL of its own. The chain accepts it while it keeps
    /// the Merkle root that the inputs prove against. The chain keeps a root
    /// for the ledger's `global_ttl` after its coin tree moves past that
    /// root, and for a fixed hour on ledger 8. Until then, the fee payer can
    /// still submit the transaction. A later build can then select the
    /// released inputs, and the chain rejects one of the two transactions.
    ///
    /// Empty for [`Self::from_proven_bytes`], and so for the `.without_dust()`
    /// of a generated contract call. The inputs of such a call stay reserved
    /// until a sync sees the chain spend them, or until their TTL passes.
    pub fn spent(&self) -> &SpentInputs {
        &self.spent
    }

    /// Consume this, returning the proven transaction bytes and
    /// [`Self::spent`].
    pub fn into_parts(self) -> (Vec<u8>, SpentInputs) {
        (self.tx_bytes, self.spent)
    }
}

/// Pending dust-address registration. See [module docs](crate::transfer) for
/// the `.await` vs `.build()` distinction.
#[derive(Debug)]
#[must_use = "does nothing until awaited or built"]
pub struct DustRegistration<'a> {
    provider: &'a MidnightProvider,
    utxo_ctime: Option<u64>,
    target: Option<DustPublicKey>,
}

impl<'a> DustRegistration<'a> {
    pub(crate) fn new(provider: &'a MidnightProvider, utxo_ctime: Option<u64>) -> Self {
        Self {
            provider,
            utxo_ctime,
            target: None,
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

    /// Build the registration transaction without submitting. Spends and
    /// re-creates one of the wallet's tNIGHT UTXOs as part of the build.
    pub async fn build(self) -> Result<TransferResult, ProviderError> {
        let provider = self.provider;
        provider.build_then_prove(self.into_request()).await
    }

    fn into_request(self) -> TransferRequest {
        TransferKind::DustRegistration {
            utxo_ctime: self.utxo_ctime,
            target: self.target,
        }
        .into()
    }
}

impl<'a> IntoFuture for DustRegistration<'a> {
    type Output = Result<PendingTx, ProviderError>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send + 'a>>;

    fn into_future(self) -> Self::IntoFuture {
        let provider = self.provider;
        Box::pin(build_and_submit(provider, self.into_request()))
    }
}

async fn build_and_submit(
    provider: &MidnightProvider,
    request: TransferRequest,
) -> Result<PendingTx, ProviderError> {
    let result = provider.build_then_prove(request).await?;
    provider.submit_reserved(result).await
}
