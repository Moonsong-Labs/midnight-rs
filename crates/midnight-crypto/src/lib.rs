//! Facade over Midnight's low-level crypto crates.
//!
//! Re-exports [`midnight_base_crypto`] and [`midnight_transient_crypto`] as
//! namespaced modules so downstream consumers (e.g. validator implementations
//! that need `PersistentHashWriter`, `EmbeddedGroupAffine`, `transient_hash`,
//! etc.) don't have to take direct dependencies on each.
//!
//! Pairs with [`midnight_helpers`], which only re-exports a curated subset of
//! these crates' types alongside its higher-level wallet helpers.

pub use midnight_base_crypto as base;
pub use midnight_transient_crypto as transient;
