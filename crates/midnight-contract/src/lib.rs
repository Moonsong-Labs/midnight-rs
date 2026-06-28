mod address;
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
pub use midnight_bindgen_runtime::ContractMaintenanceAuthority;

// Transaction-submission observability. Returned by
// `PendingDeploy::wait_best` / `wait_finalized` so callers don't need a
// separate dependency on `midnight-provider` to name the types.
// `SubmitError` is the structured failure the waits surface (inside
// `ProviderError::Submission`); `TxResultWait` is the outcome of
// `MidnightProvider::wait_transaction_result`.
pub use midnight_provider::{PendingTx, SubmitError, TxInBlock, TxResultWait};

// Re-exports for hand-building shielded offers attached to deploys (see
// `DeployBuilder::with_shielded_offer`). `OfferInfo` is the zswap "guaranteed
// offer" that rides alongside the contract action in the same transaction
// segment; `InputInfo` / `OutputInfo` are the shielded coin spend / output
// records you populate it with. `parse_shielded_recipient` decodes a
// `mn_shield-addr_*` string into the recipient type expected by
// `OutputInfo::destination`.
pub use midnight_helpers::{
    DefaultDB, InputInfo, OfferInfo, OutputInfo, ShieldedTokenType, ShieldedWallet,
};
// Recipient key types for `Circuits::with_coin_encryption_keys` / `Contract::call_with`:
// the Rust equivalent of midnight-js's `additionalCoinEncPublicKeyMappings`.
// This `coin_public_key -> encryption_public_key` mapping lets the SDK attach
// the discovery ciphertext to circuit-created shielded outputs.
pub use midnight_helpers::{CoinPublicKey, EncryptionPublicKey};
pub use midnight_wallet::parse_shielded_recipient;

/// Trait for types that can be deserialized from hex-encoded contract state.
pub trait FromHex: Sized {
    fn from_hex(hex_state: &str) -> Result<Self, midnight_bindgen_runtime::StateError>;
}

// The `state`, `call`, and `deploy` modules expose a thin sliver of the
// plumbing underneath `Contract::deploy`/`Contract::at` — `state::fetch_state`
// and `state::fetch_state_from_node` are reached from bindgen-generated code,
// and `deploy::deploy_funded` / `call::build_unproven_call_tx` are reached
// from integration tests. Everything else is `pub(crate)`.
