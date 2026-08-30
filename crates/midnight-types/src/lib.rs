//! The ledger this build speaks, and the vocabulary built on it.
//!
//! Two jobs, and the first one carries the second.
//!
//! This crate re-exports one generation of the ledger, through
//! `midnight-node-ledger-helpers`, and it is the only crate in the workspace
//! that names a generation. Everything else reaches `LedgerContext`,
//! `DustSpend`, `WalletSeed` and the ledger crates through here, so the choice
//! is made once. [`compat`] carries the few places where the generations
//! disagree.
//!
//! On top of that sits an implementation-free vocabulary for Midnight wallets:
//! the network and address model, the balance readings, the transfer build
//! machinery and its requests and results, and [`WalletError`]. No wallet
//! implementation lives here, and none is depended on, so a wallet, a
//! provider, and a contract builder can all share this vocabulary without
//! sharing an implementation.
//!
//! The API a consumer programs a wallet against is the `WalletFacade` trait in
//! `midnight-wallet-facade`, which speaks in this crate's types.
//!
//! The generation is one opt-in feature, `ledger-9`. Its absence selects
//! ledger 8, which is what preprod, preview and mainnet run, so the default
//! build targets the deployed networks. Turn the feature on for a chain that
//! has taken the newer ledger, which the testnets get first.
//!
//! One feature rather than a mutually exclusive pair, because cargo unifies
//! features across a dependency graph: a pair would make an application
//! unbuildable as soon as two of its dependencies disagreed.

#[cfg(not(feature = "ledger-9"))]
pub use midnight_node_ledger_helpers::ledger_8::*;
#[cfg(feature = "ledger-9")]
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

pub mod address;
pub mod balance;
pub mod chain_pin;
pub mod network;
pub mod prepared_input;
pub mod transfer;

mod error;
mod sync;

pub use balance::{
    DustBalance, ShieldedBalance, ShieldedCoinBalance, SpendableShieldedCoin, UnshieldedUtxoInfo,
    WalletBalance,
};
pub use error::WalletError;
pub use network::Network;
pub use prepared_input::PreparedInput;
pub use sync::{SyncCursors, TrackedUtxo, parse_intent_hash_hex};
pub use transfer::{
    BuildInputs, DustSpendBatch, PreparedTransfer, SpentInputs, SpentUtxoKey, TransferBuilder,
    TransferKind, TransferRequest, TransferResult, panic_message, parse_shielded_recipient,
};
