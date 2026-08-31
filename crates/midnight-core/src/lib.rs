//! Meta-crate for the midnight-rs SDK.
//!
//! Re-exports all sub-crates for convenience. Use feature flags to opt out
//! of crates you don't need.

#[cfg(feature = "indexer")]
pub use midnight_indexer_client as indexer;

#[cfg(feature = "provider")]
pub use midnight_provider as provider;

#[cfg(feature = "wallet")]
pub use midnight_wallet as wallet;

#[cfg(feature = "contract")]
pub use midnight_contract as contract;

#[cfg(feature = "crypto")]
pub use midnight_crypto as crypto;

// Re-export key provider types at top level.
#[cfg(feature = "provider")]
pub use midnight_provider::{
    Health, MidnightProvider, PendingTx, Provider, ProviderError, TxInBlock, Verdict,
};

// Re-export the private-state store types at top level.
#[cfg(feature = "provider")]
pub use midnight_provider::{
    ConflictStrategy, EncryptedExport, ExportOptions, FsPrivateStateProvider, ImportOptions,
    ImportResult, PrivateStateError, PrivateStateProvider, Snapshot, SnapshotStatus,
};

// Re-export key indexer types at top level.
#[cfg(feature = "indexer")]
pub use midnight_indexer_client::{
    Block, BlockOffset, BridgeClaimTransaction, ContractAction, ContractActionOffset,
    ContractBalance, ContractCall, ContractDeploy, ContractUpdate, IndexerClient, IndexerError,
    RegularTransaction, Segment, SystemTransaction, Transaction, TransactionOffset,
    TransactionResult, TransactionResultStatus, UnshieldedUtxo,
};

// Re-export the wallet types a caller names to sync one and attach it, so
// the common path needs no `midnight_core::wallet::` prefix.
#[cfg(feature = "wallet")]
pub use midnight_wallet::{LocalWallet, Seed, SyncProgress, Wallet, WalletFacade};

// Re-export contract types (gated behind "contract" feature).
#[cfg(feature = "contract")]
pub use midnight_contract::{Contract, ContractError, FromHex};

// Re-export compact-bindgen for the contract! macro (gated behind "contract" feature).
#[cfg(feature = "contract")]
pub use compact_bindgen;

/// Generate typed Rust bindings from a Compact `analyzed-ir.sexp` file.
///
/// This is a convenience wrapper around [`compact_bindgen::contract!`] that
/// automatically sets the crate path to `midnight_core::compact_bindgen`.
///
/// # Examples
///
/// ```ignore
/// // Generates `pub mod gateway { pub struct Gateway { ... } ... }`.
/// midnight_core::contract!(Gateway, "gateway-analyzed-ir.sexp");
///
/// // Flat output (struct named `Ledger`).
/// midnight_core::contract!("gateway-analyzed-ir.sexp");
/// ```
#[cfg(feature = "contract")]
#[macro_export]
macro_rules! contract {
    ($name:ident, $path:literal) => {
        $crate::compact_bindgen::contract!(
            #[crate($crate::compact_bindgen)]
            $name,
            $path
        );
    };
    ($path:literal) => {
        $crate::compact_bindgen::contract!(
            #[crate($crate::compact_bindgen)]
            $path
        );
    };
}

#[cfg(test)]
mod tests {
    #[test]
    fn reexports_provider_types() {
        let _: fn() -> Result<Option<crate::Block>, crate::ProviderError>;
        let _: fn() -> Result<Option<crate::Transaction>, crate::IndexerError>;
    }

    #[test]
    #[cfg(feature = "wallet")]
    fn reexports_the_wallet_types_a_caller_attaches() {
        // Guards the meta-crate's own surface: `wallet` re-exports the crate,
        // and these four are the names the attach path spells out.
        let _: fn(crate::Wallet) -> crate::LocalWallet = crate::LocalWallet::new;
        let _ = std::any::type_name::<crate::SyncProgress>();
        let _ = std::any::type_name::<crate::Seed>();
        let _ = std::any::type_name::<Box<dyn crate::WalletFacade>>();
    }

    #[test]
    #[cfg(feature = "contract")]
    fn reexports_contract_types() {
        use crate::{Contract, ContractError};
        let _: fn() -> Result<(), ContractError>;
        let _ = std::any::type_name::<Contract<()>>();
    }
}
