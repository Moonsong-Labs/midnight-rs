//! Facade over [`midnight_node_ledger_helpers`].
//!
//! Every other workspace crate that needs `LedgerContext`, `DustSpend`,
//! `WalletSeed`, etc. imports them from `midnight_helpers` instead of the
//! upstream helpers crate. That keeps the upstream dep pinned in exactly
//! one place (this `Cargo.toml`) so we can swap the source, vendor it, or
//! restructure feature flags without touching every consumer.
//!
//! Re-exports the upstream `pub` surface verbatim — no filtering, no
//! renames. Add new wrappers / extensions here as needed; keep the
//! re-export glob intact for everything else.

pub use midnight_node_ledger_helpers::*;
