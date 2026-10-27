//! Runtime errors surfaced while interpreting a Compact circuit IR body.

/// Error during circuit IR execution.
#[derive(Debug, thiserror::Error)]
pub enum InterpreterError {
    #[error("undefined variable: {0}")]
    UndefinedVariable(String),

    #[error("assertion failed: {0}")]
    AssertionFailed(String),

    #[error("ledger query failed: {0}")]
    LedgerQueryFailed(String),

    #[error("type error: {0}")]
    TypeError(String),

    #[error("unsupported IR node: {0}")]
    Unsupported(String),

    /// A witness call failed: the provider knew the name but could not
    /// produce a value (key store unreachable, corrupt private state, argument
    /// conversion failure, ...), or the provider has no witness with the name
    /// ([`crate::WitnessOutcome::Unknown`]). Always aborts execution.
    #[error("witness error: {0}")]
    Witness(String),
}
