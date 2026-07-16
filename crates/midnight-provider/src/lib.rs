mod error;
mod provider;
mod remote_prover;
mod submit;
pub mod transfer;
mod types;

pub use error::ProviderError;
pub use provider::{MidnightProvider, NodeBlockHash, NodeHeader, SyncHandle, SyncWalletBuilder};
pub use remote_prover::RemoteProofServer;
pub use submit::{PendingTx, PreparedTx, SubmitError, TxInBlock, Verdict};
pub use transfer::{DustRegistration, ShieldedTransfer, UnshieldedTransfer};
pub use types::{Health, StateQuery, StateQueryResult, TxResultWait};

// Re-export the wallet types that appear in MidnightProvider's public surface
// so callers don't need a separate dep on midnight-wallet for them.
pub use midnight_wallet::{
    AccountKey, HashOutput, NIGHT, Network, Role, RoleKey, SPECKS_PER_DUST, STARS_PER_NIGHT, Seed,
    SeedError, ShieldedTokenType, SyncProgress, TransferResult, UnshieldedTokenType, Wallet,
    WalletBalance, WalletError, WalletSeed, WalletSeedError, mnemonic,
};

// Re-export the private-state types so callers configure
// `MidnightProvider::with_private_state` without a separate dep.
pub use midnight_private_state::{
    ConflictStrategy, EncryptedExport, ExportOptions, FsPrivateStateProvider, ImportOptions,
    ImportResult, PrivateStateError, PrivateStateProvider, Snapshot, SnapshotStatus,
};

// Re-export indexer types so consumers of midnight-provider don't need
// a separate dependency on midnight-indexer-client for response types.
pub use midnight_indexer_client::{
    self as indexer, Block, BlockOffset, ContractAction, ContractActionOffset, ContractBalance,
    ContractCall, ContractDeploy, ContractUpdate, GraphQLError, IndexerClient, IndexerError,
    RegularTransaction, Segment, SystemTransaction, Transaction, TransactionFees,
    TransactionOffset, TransactionResult, TransactionResultStatus, UnshieldedUtxo,
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
    /// Fetch hex-encoded contract state. Returns the latest state when
    /// `offset` is `None`.
    async fn get_contract_state(
        &self,
        address: &str,
        offset: Option<ContractActionOffset>,
    ) -> Result<Option<String>, ProviderError>;

    /// Fetch the block height of the latest transaction touching a contract.
    async fn get_latest_contract_block_height(
        &self,
        address: &str,
    ) -> Result<Option<i64>, ProviderError>;

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

#[cfg(feature = "bindgen")]
mod lazy_bridge {
    use super::*;
    use midnight_typed_state::{hex, lazy};
    use sp_storage::StorageKey;

    /// Re-export so consumers can use `StateQueryProvider` without depending
    /// on `compact-bindgen` directly.
    pub use lazy::StateQueryProvider;

    impl lazy::StateQueryProvider for MidnightProvider {
        type Error = ProviderError;

        async fn query_contract_state(
            &self,
            address: &str,
            queries: Vec<lazy::StateQuery>,
            at_block_hash: Option<NodeBlockHash>,
        ) -> Result<Vec<lazy::StateQueryResult>, ProviderError> {
            // Convert bindgen hex strings → StorageKey raw bytes
            let provider_queries: Vec<StateQuery> = queries
                .into_iter()
                .map(|q| StateQuery {
                    path: q
                        .path
                        .into_iter()
                        .map(|h| StorageKey(hex::decode(&h).unwrap_or_else(|_| h.into_bytes())))
                        .collect(),
                })
                .collect();

            let results = self
                .query_contract_state_at(address, provider_queries, at_block_hash)
                .await?;

            // Convert StorageKey raw bytes → bindgen hex strings
            Ok(results
                .into_iter()
                .map(|r| lazy::StateQueryResult {
                    query: lazy::StateQuery {
                        path: r.query.path.into_iter().map(|k| hex::encode(k.0)).collect(),
                    },
                    value: r.value,
                    error: r.error,
                })
                .collect())
        }
    }
}

#[cfg(feature = "bindgen")]
pub use lazy_bridge::StateQueryProvider;

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
                offset: Option<ContractActionOffset>,
            ) -> Result<Option<String>, ProviderError> {
                (**self).get_contract_state(address, offset).await
            }
            async fn get_latest_contract_block_height(
                &self,
                address: &str,
            ) -> Result<Option<i64>, ProviderError> {
                (**self).get_latest_contract_block_height(address).await
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
            _address: &str,
            _offset: Option<ContractActionOffset>,
        ) -> Result<Option<String>, ProviderError> {
            Ok(None)
        }
        async fn get_latest_contract_block_height(
            &self,
            _address: &str,
        ) -> Result<Option<i64>, ProviderError> {
            Ok(Some(42))
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
    async fn blanket_impl_ref() {
        let p = DummyProvider;
        let r: &dyn Provider = &p;
        assert_eq!(
            r.get_latest_contract_block_height("a").await.unwrap(),
            Some(42)
        );
    }

    #[tokio::test]
    async fn blanket_impl_arc() {
        let p = Arc::new(DummyProvider);
        assert_eq!(
            p.get_latest_contract_block_height("a").await.unwrap(),
            Some(42)
        );
    }

    #[tokio::test]
    async fn blanket_impl_box() {
        let p: Box<dyn Provider> = Box::new(DummyProvider);
        assert_eq!(
            p.get_latest_contract_block_height("a").await.unwrap(),
            Some(42)
        );
    }
}
