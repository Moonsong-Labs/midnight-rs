//! The entry crate of the midnight-rs SDK, and the one crate to depend on.
//!
//! It re-exports the SDK crates as the modules [`provider`], [`wallet`],
//! [`contract`](mod@contract) and [`crypto`]. The
//! [`compact_bindgen`] module holds the value types that generated bindings
//! take, such as [`Bytes`](compact_bindgen::Bytes). Its root holds the names
//! of the common path: sync a [`Wallet`], attach it to a [`MidnightProvider`],
//! then deploy and call a contract that [`contract!`] binds. The provider funds
//! and signs through the [`MidnightWallet`] trait. The root also holds
//! [`NIGHT`], the native token, and the unit counts [`STARS_PER_NIGHT`] and
//! [`SPECKS_PER_DUST`] that a transfer request and a wallet balance name.
//!
#![cfg_attr(
    all(feature = "provider", feature = "wallet", feature = "contract"),
    doc = "```no_run"
)]
#![cfg_attr(
    not(all(feature = "provider", feature = "wallet", feature = "contract")),
    doc = "```ignore"
)]
//! use midnight_core::{MidnightProvider, Seed, Wallet};
//!
//! mod counter {
//!     midnight_core::contract!("../../devnet/contracts/counter/compiled/compiler/analyzed-ir.sexp");
//! }
//!
//! async fn run(phrase: &str) -> anyhow::Result<()> {
//!     let provider = MidnightProvider::new("ws://localhost:9944");
//!     let wallet = Wallet::from_seed(Seed::from_mnemonic(phrase)?)
//!         .sync(&provider, "http://localhost:8088")
//!         .await?;
//!     let provider = provider.with_wallet(&wallet);
//!
//!     let contract = counter::Contract::deploy(&provider)
//!         .with_zk_config(counter::ZK_CONFIG_DIR)
//!         .send()
//!         .await?
//!         .into_contract()
//!         .await?;
//!     let returned = contract
//!         .circuits()
//!         .increment()
//!         .send()
//!         .await?
//!         .get_receipt()
//!         .await?
//!         .value;
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
//! # Runtime
//!
//! The SDK runs on tokio 1. Run its futures on a tokio runtime that has the
//! I/O and time drivers enabled. Any flavor works, so a current-thread
//! runtime is enough. `#[tokio::main]` and `#[tokio::test]` build such a
//! runtime.
//!
//! Outside such a runtime, the SDK panics at the first step that sets a
//! timer, spawns a task or opens a network connection.
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

#[cfg(feature = "provider")]
pub use midnight_provider::MidnightProvider;

// A transfer request and a wallet balance both name the native token and the
// unit counts, so no feature gates them.
pub use midnight_types::{NIGHT, SPECKS_PER_DUST, STARS_PER_NIGHT};

// Re-export the wallet types a caller names to sync one and attach it, so
// the common path needs no `midnight_core::wallet::` prefix.
#[cfg(feature = "wallet")]
pub use midnight_wallet::{
    InvalidNetworkName, MidnightWallet, Network, NetworkName, Seed, SyncProgress, Wallet,
    WalletBuilder,
};

// Re-export contract types (gated behind "contract" feature).
#[cfg(feature = "contract")]
pub use midnight_contract::{Contract, ContractError};

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
    #[cfg(feature = "contract")]
    fn reexports_contract_types() {
        use crate::{Contract, ContractError};
        let _: fn() -> Result<(), ContractError>;
        let _ = std::any::type_name::<Contract<()>>();
    }
}
