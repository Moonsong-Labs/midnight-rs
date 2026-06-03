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

// `midnight-node-ledger-helpers` re-exports `MAX_SUPPLY` from
// `midnight_ledger::structure` but not its two siblings `SPECKS_PER_DUST`
// (`1 DUST = 10^15 SPECK`) and `STARS_PER_NIGHT` (`1 NIGHT = 10^6 STAR`).
// Surface them here so callers don't need to either hand-roll the literals
// or reach for `midnight_ledger` directly.
pub use midnight_ledger::structure::{SPECKS_PER_DUST, STARS_PER_NIGHT};
