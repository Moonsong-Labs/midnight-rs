//! Facade over the midnight-node ledger helpers.
//!
//! Every other workspace crate that needs `LedgerContext`, `DustSpend`,
//! `WalletSeed`, etc. imports them from `midnight_helpers` instead of the
//! upstream helpers. That keeps the upstream dependency pinned in exactly one
//! place (the root manifest's `[workspace.dependencies]`, which only this crate
//! names) so the source can move without touching every consumer.
//!
//! The crate root re-exports the whole upstream surface of ledger 9: the
//! ledger, zswap, onchain-runtime and coin-structure crates it binds, the
//! transaction and wallet builders, and the shims such as
//! `contract_operation_new`.
//!
//! [`ledger_8`] and [`fork`] exist for one task: the wallet reads the shielded
//! events that a chain made before its hard fork to ledger 9, and carries the
//! state it builds from them across that fork. Use the crate root for all
//! other work.

pub use midnight_ledger_unsafe_helpers::ledger_9::*;

// `1 DUST = 10^15 SPECK` and `1 NIGHT = 10^6 STAR`. Upstream re-exports
// their sibling `MAX_SUPPLY` but not these two.
pub use mn_ledger::structure::{SPECKS_PER_DUST, STARS_PER_NIGHT};

/// The ledger context every build in this workspace runs against.
pub type BuildContext = LedgerContext<DefaultDB>;

/// An intent before its proofs are made.
pub type UnprovenIntent<D> = Intent<Signature, ProofPreimageMarker, PedersenRandomness, D>;

/// A transaction with its proofs made and its binding not yet sealed, as
/// a [`ProofProvider`] returns it.
pub type ProvenTransaction<D> = Transaction<Signature, ProofMarker, PedersenRandomness, D>;

pub use midnight_ledger_unsafe_helpers::ContractVerifyingKeyBytes;

/// Ledger 8, which a chain ran until its hard fork to ledger 9.
pub mod ledger_8 {
    pub use midnight_ledger_unsafe_helpers::ledger_8::*;
}

/// Carrying values across the hard fork from ledger 8 to ledger 9.
pub mod fork {
    pub use midnight_ledger_unsafe_helpers::fork::fork_8_to_9::old_to_new_ser;
}
