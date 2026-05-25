pub mod address;
pub mod call;
mod contract;
pub mod deploy;
mod error;
pub mod interpreter;
pub mod maintenance;
mod prover;
mod remote_prover;
pub mod state;

// Re-exports referenced by the bindgen `contract!` macro's generated code.
// Hidden from rustdoc because they're not part of the user-facing API.
#[doc(hidden)]
pub use compact_codegen;
pub use midnight_provider::Provider;

// Primary API: deploy / connect / call.
pub use contract::{
    AsMidnightProvider, BlockRef, ConnectBuilder, Contract, DeployBuilder, PendingDeploy,
};
pub use error::ContractError;
pub use prover::Prover;

// Contract maintenance / governance (verifier-key rotation, authority
// replacement). The signature primitives are re-exported so callers can build
// committees and sign maintenance ops without depending on `midnight-base-crypto`
// directly.
pub use maintenance::{ContractMaintenance, PreparedMaintenance};
pub use midnight_base_crypto::signatures::{Signature, SigningKey, VerifyingKey};

// Transaction-submission observability. Returned by
// `PendingDeploy::wait_best` / `wait_finalized` so callers don't need a
// separate dependency on `midnight-provider` to name the types.
pub use midnight_provider::{PendingTx, TxInBlock};

// Re-exports for hand-building shielded offers attached to contract calls
// (see `Contract::call_with_shielded` and `DeployBuilder::with_shielded_offer`).
// `OfferInfo` is the zswap "guaranteed offer" that rides alongside the
// contract action in the same transaction segment; `InputInfo` / `OutputInfo`
// are the shielded coin spend / output records you populate it with.
// `parse_shielded_recipient` decodes a `mn_shield-addr_*` string into the
// recipient type expected by `OutputInfo::destination`.
pub use midnight_helpers::{
    DefaultDB, InputInfo, OfferInfo, OutputInfo, ShieldedTokenType, ShieldedWallet,
};
pub use midnight_wallet::parse_shielded_recipient;

/// Trait for types that can be deserialized from hex-encoded contract state.
pub trait FromHex: Sized {
    fn from_hex(hex_state: &str) -> Result<Self, midnight_bindgen::StateError>;
}

// Note: lower-level helpers live under their topical module
// (`midnight_contract::state::*` for state reads, `midnight_contract::deploy::*`
// for the deploy plumbing, `midnight_contract::address::*` for address utils,
// `midnight_contract::call::*` for call-side internals). They're the plumbing
// underneath `Contract::deploy/at` and not part of the supported high-level API.
