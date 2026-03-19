//! Meta-crate for the midnight-rs SDK.
//!
//! Re-exports all sub-crates for convenience. Use feature flags to opt out
//! of crates you don't need.

#[cfg(feature = "indexer")]
pub use midnight_indexer_client as indexer;

#[cfg(feature = "provider")]
pub use midnight_provider as provider;

#[cfg(feature = "contract")]
pub use midnight_contract as contract;

// Re-export key provider types at top level.
#[cfg(feature = "provider")]
pub use midnight_provider::{Health, MidnightProvider, Provider, ProviderError};

// Re-export key indexer types at top level.
#[cfg(feature = "indexer")]
pub use midnight_indexer_client::{
    Block, ContractAction, ContractBalance, ContractCall, ContractDeploy, ContractUpdate,
    IndexerClient, IndexerError, RegularTransaction, Segment, SystemTransaction, Transaction,
    TransactionFees, TransactionResult, TransactionResultStatus, UnshieldedUtxo,
};

// Re-export contract types (gated behind "contract" feature).
#[cfg(feature = "contract")]
pub use midnight_contract::{Contract, ContractError, FromHex};

// Re-export midnight-bindgen for the contract! macro (gated behind "contract" feature).
#[cfg(feature = "contract")]
pub use midnight_bindgen;

/// Generate typed Rust bindings from a Compact `contract-info.json` file.
///
/// This is a convenience wrapper around [`midnight_bindgen::contract!`] that
/// automatically sets the crate path to `midnight_core::midnight_bindgen`.
///
/// # Examples
///
/// ```ignore
/// // Generates `pub mod gateway { pub struct Gateway { ... } ... }`.
/// midnight_core::contract!(Gateway, "gateway-contract-info.json");
///
/// // Flat output (struct named `Ledger`).
/// midnight_core::contract!("gateway-contract-info.json");
/// ```
#[cfg(feature = "contract")]
#[macro_export]
macro_rules! contract {
    ($name:ident, $path:literal) => {
        $crate::midnight_bindgen::contract!(
            #[crate($crate::midnight_bindgen)]
            $name,
            $path
        );
    };
    ($path:literal) => {
        $crate::midnight_bindgen::contract!(
            #[crate($crate::midnight_bindgen)]
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
    #[cfg(feature = "contract")]
    fn reexports_contract_types() {
        use crate::{Contract, ContractError};
        let _: fn() -> Result<(), ContractError>;
        let _ = std::any::type_name::<Contract<(), ()>>();
    }
}
