mod error;
pub mod ledger_8;
#[expect(
    clippy::duplicate_mod,
    reason = "`ledger_8` and `ledger_9` compile the same per-ledger source against each generation"
)]
pub mod ledger_9;
pub mod node_error;
mod proof_providers;
mod provider;
mod remote_prover;
mod submit;
pub mod transfer;
mod types;

pub use error::ProviderError;
pub use proof_providers::ProofProviders;
pub use provider::{Builds, HeldInputs, MidnightProvider, NodeHeader};
pub use remote_prover::RemoteProofServer;
pub use submit::{NotApplied, PendingTx, PreparedTx, SubmitError, TxInBlock, Verdict};
pub use transfer::{
    DustRegistration, DustlessBuilder, DustlessTransaction, ShieldedSwap, ShieldedTransfer,
    UnshieldedTransfer,
};
pub use types::{Health, TransactionHash};

// Re-export the vocabulary that appears in MidnightProvider's public surface
// so callers don't need separate deps for it. The implementation types
// (`Wallet`, `LocalWallet`, `WalletBuilder`) live in midnight-wallet; this
// crate never names them.
pub use midnight_types::{
    BlockHash, ChainParameters, CoinInfo, CoinPublicKey, CoinSelectionStrategy, ContractAddress,
    DustParameters, EncryptionPublicKey, ExtrinsicHash, HashOutput, LedgerVersion, NIGHT, Nonce,
    Nullifier, SPECKS_PER_DUST, STARS_PER_NIGHT, ShieldedRecipient, ShieldedTokenType,
    UnshieldedTokenType,
};

// The wallet's API, so a caller attaching one (or implementing one) needs no
// separate dep on midnight-wallet-facade.
pub use midnight_types::hd::{Seed, SeedError};
pub use midnight_types::{
    Network, ShieldedCoinBalance, SpendableShieldedCoin, SpentInputs, SpentUtxoKey, SyncCursors,
    TrackedUtxo, TransferKind, TransferRequest, TransferResult, WalletBalance, WalletError,
};
pub use midnight_wallet_facade::{IntoWalletFacade, WalletFacade};

// Re-export the private-state types so callers configure
// `MidnightProvider::with_private_state` without a separate dep.
pub use midnight_private_state::{
    ConflictStrategy, EncryptedExport, ExportOptions, FsPrivateStateProvider, ImportOptions,
    ImportResult, PrivateStateError, PrivateStateProvider, Snapshot, SnapshotStatus,
};

// Re-export the state query model that `Provider` and the generated lazy
// accessors share, so callers need no separate dep on midnight-typed-state.
pub use midnight_typed_state::lazy::{
    StateQuery, StateQueryProvider, StateQueryResult, StorageKey,
};

use std::sync::Arc;

/// Read-only interface to the Midnight network.
///
/// The trait is intentionally narrow: only the methods invoked through a
/// `<P: Provider>` bound live here, so a new backend has a small,
/// well-defined surface to implement. Reachability checks
/// (`MidnightProvider::health`, block / network metadata, etc.) are
/// inherent methods on [`MidnightProvider`] — they aren't part of the
/// generic abstraction.
///
/// Automatically implemented for `&T`, `Arc<T>`, and `Box<T>` where `T: Provider`.
#[async_trait::async_trait]
pub trait Provider: Send + Sync {
    /// Fetch the hex-encoded state of the contract at `address`, as of the
    /// block `at`.
    ///
    /// When `at` is `None`, read at the node's best block. A reorg can replace
    /// a best block, so pass a finalized block hash for state that cannot
    /// change.
    ///
    /// Returns `None` when no contract exists at `address` in that block.
    async fn get_contract_state(
        &self,
        address: &str,
        at: Option<BlockHash>,
    ) -> Result<Option<String>, ProviderError>;

    /// Query specific fields/keys in a contract's state tree without
    /// downloading the entire state blob.
    async fn query_contract_state(
        &self,
        address: &str,
        queries: Vec<StateQuery>,
    ) -> Result<Vec<StateQueryResult>, ProviderError>;
}

// ---------------------------------------------------------------------------
// Bridge: MidnightProvider → StateQueryProvider (from compact-bindgen)
// ---------------------------------------------------------------------------

impl StateQueryProvider for MidnightProvider {
    type Error = ProviderError;

    async fn query_contract_state(
        &self,
        address: &str,
        queries: Vec<StateQuery>,
        at_block_hash: Option<BlockHash>,
    ) -> Result<Vec<StateQueryResult>, ProviderError> {
        self.query_contract_state_at(address, queries, at_block_hash)
            .await
    }
}

// ---------------------------------------------------------------------------
// Blanket impls for &T, Arc<T>, Box<T>
// ---------------------------------------------------------------------------

macro_rules! delegate_provider {
    ($($wrapper:ty),+ $(,)?) => { $(
        #[async_trait::async_trait]
        impl<T: Provider + ?Sized> Provider for $wrapper {
            async fn get_contract_state(
                &self,
                address: &str,
                at: Option<BlockHash>,
            ) -> Result<Option<String>, ProviderError> {
                (**self).get_contract_state(address, at).await
            }
            async fn query_contract_state(
                &self,
                address: &str,
                queries: Vec<StateQuery>,
            ) -> Result<Vec<StateQueryResult>, ProviderError> {
                (**self).query_contract_state(address, queries).await
            }
        }
    )+ };
}

delegate_provider!(&T, Arc<T>, Box<T>);

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    struct DummyProvider;

    #[async_trait::async_trait]
    impl Provider for DummyProvider {
        async fn get_contract_state(
            &self,
            address: &str,
            at: Option<BlockHash>,
        ) -> Result<Option<String>, ProviderError> {
            Ok(Some(format!("{address}@{at:?}")))
        }
        async fn query_contract_state(
            &self,
            _address: &str,
            _queries: Vec<StateQuery>,
        ) -> Result<Vec<StateQueryResult>, ProviderError> {
            Ok(vec![])
        }
    }

    #[tokio::test]
    async fn blanket_impl_arc() {
        let p = Arc::new(DummyProvider);
        let at = Some(BlockHash::repeat_byte(0xab));
        assert_eq!(
            p.get_contract_state("a", at).await.unwrap(),
            DummyProvider.get_contract_state("a", at).await.unwrap()
        );
    }

    #[tokio::test]
    async fn blanket_impl_box() {
        let p: Box<dyn Provider> = Box::new(DummyProvider);
        let at = Some(BlockHash::repeat_byte(0xab));
        assert_eq!(
            p.get_contract_state("a", at).await.unwrap(),
            DummyProvider.get_contract_state("a", at).await.unwrap()
        );
    }
}
