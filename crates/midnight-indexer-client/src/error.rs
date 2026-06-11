use crate::types::GraphQLError;

fn format_graphql_errors(errors: &[GraphQLError]) -> String {
    errors
        .iter()
        .map(|e| e.message.as_str())
        .collect::<Vec<_>>()
        .join("; ")
}

#[derive(Debug, thiserror::Error)]
pub enum IndexerError {
    #[error("HTTP client configuration error: {0}")]
    Config(String),

    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("GraphQL errors: {}", format_graphql_errors(.0))]
    GraphQL(Vec<GraphQLError>),

    /// A connection-level failure: connect/handshake timeout, read error,
    /// idle timeout, or the server dropping the socket. Retryable — callers
    /// may reconnect and resume from their own cursor.
    #[error("WebSocket transport error: {0}")]
    Transport(String),

    /// The server violated (or terminated) the `graphql-transport-ws`
    /// protocol: an unexpected message instead of `connection_ack`, or an
    /// `error` message for the subscription (bad query/variables). Not
    /// retryable — repeating the same request will fail the same way.
    #[error("WebSocket protocol error: {0}")]
    Protocol(String),

    #[error("deserialization error: {0}")]
    Deserialization(String),

    #[error("missing response data")]
    MissingData,
}

impl IndexerError {
    /// Whether the error is a transient connection-level failure that a
    /// caller may reasonably retry (with its own backoff and cursor-resume
    /// policy). Protocol violations, GraphQL errors, and deserialization
    /// failures are deterministic and excluded.
    pub fn is_retryable(&self) -> bool {
        matches!(self, IndexerError::Transport(_))
    }
}
