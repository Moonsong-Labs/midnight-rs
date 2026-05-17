pub mod call;
mod contract;
mod error;
pub mod interpreter;
mod prover;

// Re-export for generated code
pub use compact_codegen;
pub use midnight_provider::Provider;
pub use midnight_wallet::Wallet;

// Primary API
pub use contract::{
    AsMidnightProvider, BlockRef, ConnectBuilder, Contract, DeployBuilder, PendingDeploy,
};
pub use error::ContractError;
pub use prover::Prover;

// Lower-level building blocks
pub use call::{
    DEFAULT_TTL, DEFAULT_TX_POLL_INTERVAL, DEFAULT_TX_TIMEOUT, DeployResult, PendingTx, TxInBlock,
    call_funded, call_funded_with, call_funded_with_state, deploy_funded, deploy_funded_with_state,
    deploy_local, deserialize_state, fetch_state, fetch_state_at, fetch_state_from_node,
    format_address, parse_address, submit, sync_or_fetch_context, wait_for_contract_update,
    wait_for_deployment, with_zk_keys,
};

/// Trait for types that can be deserialized from hex-encoded contract state.
pub trait FromHex: Sized {
    fn from_hex(hex_state: &str) -> Result<Self, midnight_bindgen::StateError>;
}
