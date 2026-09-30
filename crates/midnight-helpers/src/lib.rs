//! Facade over the midnight-node ledger helpers.
//!
//! Every other workspace crate that needs `LedgerContext`, `DustSpend`,
//! `WalletSeed`, etc. imports them from `midnight_helpers` instead of the
//! upstream helpers. That keeps the upstream dependency pinned in exactly one
//! place (this crate's manifest) so the source can move without touching
//! every consumer.
//!
//! Each ledger generation is one module, [`ledger_8`] and [`ledger_9`]. A
//! module re-exports that generation's whole upstream surface: the ledger,
//! zswap, onchain-runtime and coin-structure crates it binds, the transaction
//! and wallet builders, and the per-generation shims such as
//! `contract_operation_new`. The two modules name the same items, so code
//! written against one compiles against the other.
//!
//! The crate root holds only what is one type in both generations. Upstream
//! defines some of these items once for every generation. The others come
//! from the crates that both generations link as a single instance:
//! base-crypto, serialize, storage and zkir.

/// Ledger generation 8, which mainnet, preprod and preview run until their
/// hard fork.
pub mod ledger_8 {
    pub use midnight_ledger_unsafe_helpers::ledger_8::*;

    // `1 DUST = 10^15 SPECK` and `1 NIGHT = 10^6 STAR`. Upstream re-exports
    // their sibling `MAX_SUPPLY` but not these two.
    pub use mn_ledger::structure::{SPECKS_PER_DUST, STARS_PER_NIGHT};

    /// The ledger context every build in this workspace runs against.
    pub type BuildContext = LedgerContext<DefaultDB>;
}

/// Ledger generation 9, which a chain runs from the 8 to 9 hard fork on.
pub mod ledger_9 {
    pub use midnight_ledger_unsafe_helpers::ledger_9::*;

    // `1 DUST = 10^15 SPECK` and `1 NIGHT = 10^6 STAR`. Upstream re-exports
    // their sibling `MAX_SUPPLY` but not these two.
    pub use mn_ledger::structure::{SPECKS_PER_DUST, STARS_PER_NIGHT};

    /// The ledger context every build in this workspace runs against.
    pub type BuildContext = LedgerContext<DefaultDB>;
}

pub use midnight_ledger_unsafe_helpers::{CoinSelectionStrategy, ContractVerifyingKeyBytes};

/// Carrying values across the hard fork from ledger 8 to ledger 9.
pub mod fork {
    pub use midnight_ledger_unsafe_helpers::fork::fork_8_to_9::old_to_new_ser;
}

pub use ledger_9::{
    AlignedValue, DB, DefaultDB, Deserializable, Duration, HashOutput, Serializable, SigningKey,
    Sp, SplittableRng, StdRng, Tagged, Timestamp, VerifyingKey, base_crypto, midnight_serialize,
};
