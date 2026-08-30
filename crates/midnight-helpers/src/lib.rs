//! Facade over [`midnight_node_ledger_helpers`].
//!
//! Every other workspace crate that needs `LedgerContext`, `DustSpend`,
//! `WalletSeed`, etc. imports them from `midnight_helpers` instead of the
//! upstream helpers crate. That keeps the upstream dep pinned in exactly
//! one place (this `Cargo.toml`) so we can swap the source, vendor it, or
//! restructure feature flags without touching every consumer.
//!
//! This is also the only crate in the workspace that knows which ledger
//! generation it is compiled against. It re-exports one generation's `pub`
//! surface verbatim, and [`compat`] carries the few places where the
//! generations disagree. Everything else calls through both and never names
//! a generation.
//!
//! The generation is one opt-in feature, `ledger-8`, whose absence selects the
//! current generation. One feature rather than a mutually exclusive pair,
//! because cargo unifies features across a dependency graph: a pair would make
//! an application unbuildable as soon as two of its dependencies disagreed.

#[cfg(feature = "ledger-8")]
pub use midnight_node_ledger_helpers::ledger_8::*;
#[cfg(not(feature = "ledger-8"))]
pub use midnight_node_ledger_helpers::ledger_9::*;

// Declared at the upstream crate root rather than inside a generation, so the
// glob above does not carry them.
pub use midnight_node_ledger_helpers::{CoinSelectionStrategy, ContractVerifyingKeyBytes};

// Upstream re-exports `MAX_SUPPLY` but not these two siblings
// (`1 DUST = 10^15 SPECK`, `1 NIGHT = 10^6 STAR`), so callers would otherwise
// hand-roll the literals.
pub use mn_ledger::structure::{SPECKS_PER_DUST, STARS_PER_NIGHT};

pub mod compat;

/// The context the transaction builders in this workspace run against.
///
/// Upstream takes the context as a type parameter, so the same builders can
/// also run against an indexer. This workspace always builds against the
/// ledger, so it names that choice once here instead of at every builder type.
pub type BuilderCtx = LedgerContext<DefaultDB>;
