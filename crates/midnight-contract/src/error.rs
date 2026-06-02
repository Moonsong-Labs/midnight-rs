use midnight_provider::ProviderError;

/// Unified error type for all contract operations: query, call, deploy, submit.
#[derive(Debug, thiserror::Error)]
pub enum ContractError {
    #[error("provider error: {0}")]
    Provider(#[from] ProviderError),

    #[error("contract not found at address {0}")]
    NotFound(String),

    #[error("state deserialization error: {0}")]
    State(#[from] midnight_bindgen_runtime::StateError),

    #[error("interpreter error: {0}")]
    Interpreter(#[from] crate::interpreter::InterpreterError),

    #[error("private state error: {0}")]
    PrivateState(#[from] midnight_provider::PrivateStateError),

    #[error("transaction construction failed: {0}")]
    Construction(String),

    #[error("serialization failed: {0}")]
    Serialization(String),

    #[error("state fetch failed: {0}")]
    StateFetch(String),

    #[error("invalid address: {0}")]
    InvalidAddress(String),

    #[error("submission failed: {0}")]
    Submission(String),

    /// The transaction was included in a block (guaranteed phase passed) but
    /// the fallible phase, which carries contract calls and verifier-key
    /// updates, reported a non-`Success` status. The contract state did not
    /// advance on chain and any local private-state mutations from this call
    /// were discarded. Inspect `status` for the on-chain verdict.
    #[error(
        "transaction {extrinsic_hash} landed but the fallible phase reported {status:?}; \
         contract state did not advance"
    )]
    TransactionFailed {
        status: midnight_provider::TransactionResultStatus,
        extrinsic_hash: String,
    },

    #[error("maintenance error: {0}")]
    Maintenance(String),
}
