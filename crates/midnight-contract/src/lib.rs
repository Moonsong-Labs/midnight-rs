mod address;
pub mod call;
mod contract;
pub mod deploy;
mod error;
// The Compact IR interpreter now lives in the `compact-interpreter` crate;
// aliased here so `midnight_contract::interpreter::*` paths keep resolving.
pub use compact_interpreter as interpreter;
pub mod maintenance;
pub mod state;
pub mod zk_config;

// Re-exports referenced by the bindgen `contract!` macro's generated code.
// Hidden from rustdoc because they're not part of the user-facing API.
#[doc(hidden)]
pub use compact_codegen;
pub use midnight_provider::{NodeBlockHash, Provider};

// Primary API: deploy / connect / call.
pub use call::{CircuitDefs, ShieldedInputs};
pub use contract::{AsMidnightProvider, ConnectBuilder, Contract, DeployBuilder, PendingDeploy};
pub use error::ContractError;
pub use zk_config::{
    FsZkConfigProvider, IntoZkConfig, ZkArtifacts, ZkConfigError, ZkConfigProvider,
};

// Typed contract addresses. `ContractAddress` is re-exported so callers can
// hold and validate addresses without depending on `midnight-coin-structure`;
// `parse_address` / `format_address` convert to and from the hex form used at
// the SDK's string boundaries (`Contract::address`). `Contract::at` accepts
// either form via `IntoAddress`, and `address_serde` (de)serializes the typed
// address as hex for use in config structs.
pub use address::{IntoAddress, address_serde, format_address, parse_address};
pub use midnight_coin_structure::contract::ContractAddress;

// Contract maintenance / governance (verifier-key rotation, authority
// replacement). The signature primitives are re-exported so callers can build
// committees and sign maintenance ops without depending on `midnight-base-crypto`
// directly.
pub use maintenance::{ContractMaintenance, PreparedMaintenance};
pub use midnight_base_crypto::signatures::{Signature, SigningKey, VerifyingKey};
pub use midnight_typed_state::ContractMaintenanceAuthority;

// The execution-runtime primitives (Value domain, witnesses, execution
// results, builtins, type-aware encoding) live in `compact-runtime`.
// Re-exported as `midnight_contract::runtime` so generated bindings and callers
// reach them through one honest path instead of the interpreter module.
pub use compact_runtime as runtime;

// Transaction-submission observability. Returned by
// `PendingDeploy::wait_best` / `wait_finalized` so callers don't need a
// separate dependency on `midnight-provider` to name the types.
// `SubmitError` is the structured failure the waits surface (inside
// `ProviderError::Submission`); `TxResultWait` is the outcome of
// `MidnightProvider::wait_transaction_result`.
pub use midnight_provider::{PendingTx, SubmitError, TxInBlock, TxResultWait};
// Dustless (fee-less) build support, so generated contract-call builders can
// offer `.without_fees()` producing a sponsorable transaction.
pub use midnight_provider::{DustlessBuilder, DustlessTransaction, WithoutFees};

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
// Recipient key types for `Circuits::with_coin_encryption_keys` / `Contract::call_with`: a
// `coin_public_key -> encryption_public_key` mapping that lets the SDK attach
// the discovery ciphertext to circuit-created shielded outputs.
pub use midnight_helpers::{CoinPublicKey, EncryptionPublicKey};
pub use midnight_wallet::parse_shielded_recipient;
// The coin type callers pass to `Circuits::with_shielded_inputs` /
// `ShieldedInputs::coins` (enumerated via `MidnightProvider::spendable_shielded_coins`).
pub use midnight_wallet::SpendableShieldedCoin;

/// Trait for types that can be deserialized from hex-encoded contract state.
pub trait FromHex: Sized {
    fn from_hex(hex_state: &str) -> Result<Self, midnight_typed_state::StateError>;
}

// The `state`, `call`, and `deploy` modules expose a thin sliver of the
// plumbing underneath `Contract::deploy`/`Contract::at` — `state::fetch_state`
// and `state::fetch_state_from_node` are reached from bindgen-generated code,
// and `deploy::deploy_funded` / `call::build_unproven_call_tx` are reached
// from integration tests. Everything else is `pub(crate)`.
