//! The wallet that a `MidnightProvider` holds.

use std::sync::Arc;

use async_trait::async_trait;

use crate::chain_pin::ChainView;
use crate::{FilledTx, FundedTx, ProvenTx, ShieldedRecipient, TxEnvelope, WalletError};

/// The wallet that a `MidnightProvider` holds.
///
/// The provider reads one key, funds and signs through it. The provider proves
/// and sends, and the wallet does neither. A transaction moves through the
/// stages in order: the provider fills a [`FilledTx`], the wallet funds it
/// into a [`FundedTx`], the provider proves it into a [`ProvenTx`], and the
/// wallet signs it into a [`TxEnvelope`].
///
/// The trait has no release method. [`Self::fund_transaction`] reserves what
/// it spends, and the reservation travels with the stages. A drop of an
/// unsent stage hands the inputs back. A sent transaction keeps them until a
/// sync sees the chain spend them, until their TTL passes, or until the node
/// rejects the transaction.
///
/// `Wallet` in `midnight-wallet` implements it. So do `&T` and `Arc<T>` of an
/// implementation, so a caller can attach a borrowed or a shared wallet and
/// keep its own handle.
///
/// [`FilledTx`] and [`FundedTx`] are opaque, so a custom wallet funds through
/// a wrapped `Wallet`.
#[async_trait]
pub trait MidnightWallet: std::fmt::Debug + Send + Sync {
    /// The keys that `ownPublicKey()` and this wallet's own shielded outputs
    /// use.
    fn shielded_recipient(&self) -> ShieldedRecipient;

    /// Resync against `chain`. Then add this wallet's inputs, change and Dust
    /// fee to `tx`, and reserve what they spend.
    ///
    /// The selection and the reservation are one step, so two calls cannot
    /// spend the same input. On an error, the wallet reserves nothing.
    ///
    /// # Errors
    ///
    /// The error of the resync, a [`WalletError::ChainMismatch`] among them.
    /// [`WalletError::InsufficientShielded`],
    /// [`WalletError::InsufficientUnshielded`] and
    /// [`WalletError::InsufficientDust`] when this wallet holds too little.
    /// [`WalletError::InputsReserved`] when another transaction holds a coin
    /// that `tx` pins, and [`WalletError::DuplicateInput`] when `tx` pins a
    /// coin twice. [`WalletError::TtlOutOfRange`] and
    /// [`WalletError::TtlWithoutIntent`] for a TTL that `tx` cannot carry, and
    /// [`WalletError::Transfer`] when the build fails.
    async fn fund_transaction(
        &self,
        chain: &dyn ChainView,
        tx: FilledTx,
    ) -> Result<FundedTx, WalletError>;

    /// Add the signatures of this wallet that `tx` still needs. Then keep the
    /// reservation that `tx` carries across a restart.
    ///
    /// The provider calls it after the proof, so a process that stops during
    /// the proof leaves no reservation on disk.
    ///
    /// # Errors
    ///
    /// [`WalletError::Transfer`] when the signed transaction does not
    /// serialize.
    async fn sign_transaction(&self, tx: ProvenTx) -> Result<TxEnvelope, WalletError>;
}

#[async_trait]
impl<T: MidnightWallet + ?Sized> MidnightWallet for &T {
    fn shielded_recipient(&self) -> ShieldedRecipient {
        (**self).shielded_recipient()
    }

    async fn fund_transaction(
        &self,
        chain: &dyn ChainView,
        tx: FilledTx,
    ) -> Result<FundedTx, WalletError> {
        (**self).fund_transaction(chain, tx).await
    }

    async fn sign_transaction(&self, tx: ProvenTx) -> Result<TxEnvelope, WalletError> {
        (**self).sign_transaction(tx).await
    }
}

#[async_trait]
impl<T: MidnightWallet + ?Sized> MidnightWallet for Arc<T> {
    fn shielded_recipient(&self) -> ShieldedRecipient {
        (**self).shielded_recipient()
    }

    async fn fund_transaction(
        &self,
        chain: &dyn ChainView,
        tx: FilledTx,
    ) -> Result<FundedTx, WalletError> {
        (**self).fund_transaction(chain, tx).await
    }

    async fn sign_transaction(&self, tx: ProvenTx) -> Result<TxEnvelope, WalletError> {
        (**self).sign_transaction(tx).await
    }
}
