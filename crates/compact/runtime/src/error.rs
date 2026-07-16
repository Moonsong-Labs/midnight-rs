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

    /// A genuine witness-level failure: the provider knew the name but could
    /// not produce a value (key store unreachable, corrupt private state,
    /// argument conversion failure, ...), or nothing — provider, builtin, or
    /// helper — could handle a witness call at all. Always aborts execution;
    /// "the provider doesn't implement this name" is NOT an error, it's
    /// [`crate::WitnessOutcome::Unknown`].
    #[error("witness error: {0}")]
    Witness(String),
}
