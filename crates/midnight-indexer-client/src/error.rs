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
    /// Whether a caller can retry the failed operation.
    ///
    /// The client does not retry by itself. The caller owns the backoff and,
    /// for a subscription, the cursor to resume from.
    ///
    /// A WebSocket transport failure is retryable. That includes the connect,
    /// handshake and idle timeouts, and an upgrade that the server refuses
    /// with any HTTP status. An HTTP query is retryable when it failed before
    /// a response arrived. Examples are a DNS, TCP or TLS connect failure,
    /// and a connection that the peer reset or closed. A query that got an
    /// HTTP 5xx status is retryable too. A gateway in front of the indexer
    /// can answer 502 or 503 while the indexer restarts.
    ///
    /// A query that timed out is not retryable, because it already used the
    /// whole client timeout. A query that got any other status is not
    /// retryable. Protocol violations, GraphQL errors and deserialization
    /// failures are not retryable either.
    pub fn is_retryable(&self) -> bool {
        match self {
            IndexerError::Transport(_) => true,
            IndexerError::Http(e) => {
                (e.is_request() && !e.is_timeout())
                    || e.status().is_some_and(|s| s.is_server_error())
            }
            _ => false,
        }
    }
}
