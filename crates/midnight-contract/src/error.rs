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

    /// The transaction landed in a finalized block but the chain didn't
    /// apply it. `status` is `"PartialSuccess"` (guaranteed phase committed,
    /// at least one fallible segment failed) or `"Failure"` (whole dispatch
    /// rejected, nothing on chain). For `Contract::call_with`, the orphan
    /// `Pending` snapshot has already been cascade-dropped via `mark_failed`
    /// by the time the caller sees this error.
    #[error(
        "transaction {} landed on chain but the fallible phase reported {status}; \
         no state advance",
        hex::encode(extrinsic_hash)
    )]
    TransactionFailed {
        extrinsic_hash: [u8; 32],
        status: String,
    },

    #[error("maintenance error: {0}")]
    Maintenance(String),
}
