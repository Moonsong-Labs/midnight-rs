mod address;
mod call;
mod change;
mod contract;
pub mod deploy;
mod error;
mod ledger_8;
#[expect(
    clippy::duplicate_mod,
    reason = "`ledger_8` and `ledger_9` compile the same per-ledger source against each generation"
)]
mod ledger_9;
// The Compact IR interpreter now lives in the `compact-interpreter` crate;
// aliased here so `midnight_contract::interpreter::*` paths keep resolving.
pub use compact_interpreter as interpreter;
pub mod local;
pub mod maintenance;
mod offer;
mod resolver;
pub mod state;
pub mod zk_config;

// Re-exports referenced by the bindgen `contract!` macro's generated code.
// Hidden from rustdoc because they're not part of the user-facing API.
#[doc(hidden)]
pub use compact_codegen;
pub use midnight_provider::Provider;
pub use midnight_types::{BlockHash, ExtrinsicHash};

// Primary API: deploy / connect / call.
pub use call::ShieldedInputs;
pub use contract::{
    AsMidnightProvider, CallOutcome, ConnectBuilder, Contract, DeployBuilder, PendingCall,
    PendingDeploy, Simulated,
};
pub use error::ContractError;
pub use zk_config::{
    FsZkConfigProvider, IntoZkConfig, ZkArtifacts, ZkConfigError, ZkConfigProvider,
};

// Typed contract addresses. The crate re-exports `ContractAddress`, so a
// caller can hold and validate a contract address without a dependency on
// `midnight-types`. `parse_contract_address` and `format_contract_address`
// convert a contract address to and from the hex form at the SDK's string
// boundaries (`Contract::address`). `Contract::at` accepts either form through
// `IntoContractAddress`. `contract_address_serde` (de)serializes a typed
// contract address as hex for config structs.
pub use address::{
    IntoContractAddress, contract_address_serde, format_contract_address, parse_contract_address,
};
pub use midnight_types::ContractAddress;

// Contract maintenance / governance (verifier-key rotation, authority
// replacement). The signature primitives are re-exported so callers can build
// committees and sign maintenance ops without depending on `midnight-base-crypto`
// directly.
pub use maintenance::{ContractMaintenance, PreparedMaintenance};
pub use midnight_base_crypto::signatures::{Signature, SigningKey, VerifyingKey};
pub use midnight_typed_state::{ContractMaintenanceAuthority, ContractMaintenanceVerifyingKey};

// The execution-runtime primitives (Value domain, witnesses, execution
// results, builtins, type-aware encoding) live in `compact-runtime`.
// Re-exported as `midnight_contract::runtime` so generated bindings and callers
// reach them through one honest path instead of the interpreter module.
pub use compact_runtime as runtime;

// Transaction-submission observability. Returned by
// `PendingDeploy::wait_best` / `wait_finalized` so callers don't need a
// separate dependency on `midnight-provider` to name the types.
// `SubmitError` is the structured failure the waits surface (inside
// `ProviderError::Submission`). `NotApplied` and `Verdict` let a caller match
// `ContractError::TransactionFailed`.
pub use midnight_provider::{
    NotApplied, PendingTx, SubmitError, TransactionHash, TxInBlock, Verdict,
};
// Dustless (fee-less) build support, so generated contract-call builders can
// offer `.without_dust()` producing a sponsorable transaction.
pub use midnight_provider::{DustlessBuilder, DustlessTransaction};
// The result of a build, so callers and generated code name it without a
// dependency on `midnight-types`.
pub use midnight_types::TransferResult;

// Shielded recipients, their keys, and the shielded offer of a deploy.
pub use midnight_types::address::parse_shielded_recipient;
pub use midnight_types::{
    CoinPublicKey, EncryptionPublicKey, ShieldedRecipient, ShieldedTokenType,
};
pub use offer::{ShieldedOffer, ShieldedPayment};
// The shielded coin a caller spends, from `MidnightProvider::spendable_shielded_coins`.
pub use midnight_types::SpendableShieldedCoin;

// The `state` and `deploy` modules expose a small part of the code under
// `Contract::deploy` and `Contract::at`. Bindgen-generated code reaches
// `state::fetch_state`, and integration tests reach `deploy::deploy_funded`
// and its `DeployResult`. Everything else stays inside the crate.
