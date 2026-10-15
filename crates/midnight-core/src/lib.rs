//! The entry crate of the midnight-rs SDK, and the one crate to depend on.
//!
//! It re-exports the SDK crates as the modules [`provider`], [`wallet`],
//! [`contract`](mod@contract) and [`crypto`]. The
//! [`compact_bindgen`] module holds the value types that generated bindings
//! take, such as [`Bytes`](compact_bindgen::Bytes). Its root holds the names
//! of the common path: sync a [`Wallet`], attach it to a [`MidnightProvider`],
//! then deploy and call a contract that [`contract!`] binds.
//!
#![cfg_attr(
    all(feature = "provider", feature = "wallet", feature = "contract"),
    doc = "```no_run"
)]
#![cfg_attr(
    not(all(feature = "provider", feature = "wallet", feature = "contract")),
    doc = "```ignore"
)]
//! use midnight_core::{MidnightProvider, Wallet};
//!
//! mod counter {
//!     midnight_core::contract!("../../devnet/contracts/counter/compiled/compiler/analyzed-ir.sexp");
//! }
//!
//! async fn run(phrase: &str) -> anyhow::Result<()> {
//!     let provider = MidnightProvider::new("ws://localhost:9944", "http://localhost:8088");
//!     let wallet = Wallet::from_mnemonic(phrase)?.sync(&provider).await?;
//!     let provider = provider.with_wallet(wallet);
//!
//!     let contract = counter::Contract::deploy(&provider)
//!         .with_initial_state(counter::LedgerInitialState::default())
//!         .with_zk_config(counter::ZK_CONFIG_DIR)
//!         .await?;
//!     let returned = contract.circuits().increment().await?.value;
//!     println!("increment returned {returned}");
//!     Ok(())
//! }
//! # fn main() {}
//! ```
//!
//! [`contract!`] also emits `ZK_CONFIG_DIR`, the compiler's output directory
//! for the contract, which `with_zk_config` reads. [`compact_bindgen::contract!`]
//! states when it emits the const.
//!
//! # Features
//!
//! The four SDK modules each have a feature of the same name, and all four are
//! on by default. [`contract!`] and [`compact_bindgen`] need the `contract`
//! feature, so a build with `default-features = false` must turn that feature
//! on to bind a contract.

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
    Health, MidnightProvider, NotApplied, PendingTx, Provider, ProviderError, TxInBlock, Verdict,
};

// Re-export the private-state store types at top level.
#[cfg(feature = "provider")]
pub use midnight_provider::{
    ConflictStrategy, EncryptedExport, ExportOptions, FsPrivateStateProvider, ImportOptions,
    ImportResult, PrivateStateError, PrivateStateProvider, Snapshot, SnapshotStatus,
};

// Re-export the wallet types a caller names to sync one and attach it, so
// the common path needs no `midnight_core::wallet::` prefix.
#[cfg(feature = "wallet")]
pub use midnight_wallet::{Network, Seed, SyncProgress, Wallet, WalletBuilder, WalletFacade};

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
/// The path is relative to the `CARGO_MANIFEST_DIR` of the calling crate.
///
/// # Examples
///
/// ```
/// // Generates `pub mod counter { pub struct Counter { ... } ... }`.
/// midnight_core::contract!(
///     Counter,
///     "../midnight-contract/tests/fixtures/counter/compiler/analyzed-ir.sexp"
/// );
///
/// // Flat output (struct named `Ledger`).
/// midnight_core::contract!("../midnight-contract/tests/fixtures/counter/compiler/analyzed-ir.sexp");
/// # fn main() {}
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
    #[cfg(feature = "provider")]
    fn reexports_provider_types() {
        let _: fn() -> Result<(), crate::ProviderError>;
    }

    #[test]
    #[cfg(feature = "wallet")]
    fn reexports_the_wallet_facade() {
        // Guard only `WalletFacade`: the examples compile the other root wallet names.
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
