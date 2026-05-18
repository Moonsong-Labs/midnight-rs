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

    #[error("WebSocket transport error: {0}")]
    Transport(String),

    #[error("deserialization error: {0}")]
    Deserialization(String),

    #[error("missing response data")]
    MissingData,
}
