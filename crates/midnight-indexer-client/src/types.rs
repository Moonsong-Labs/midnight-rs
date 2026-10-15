use serde::{Deserialize, Serialize};

use crate::error::IndexerError;

// ---------------------------------------------------------------------------
// GraphQL envelope
// ---------------------------------------------------------------------------

/// Wrapper for GraphQL response envelope.
#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct GraphQLResponse<T> {
    pub data: Option<T>,
    #[serde(default)]
    pub errors: Option<Vec<GraphQLError>>,
}

impl<T> GraphQLResponse<T> {
    /// Returns the `data`, or [`IndexerError::GraphQL`] when `errors` is non-empty.
    ///
    /// The errors win over a partial result that carries both.
    pub(crate) fn into_result(self) -> Result<Option<T>, IndexerError> {
        match self.errors {
            Some(errors) if !errors.is_empty() => Err(IndexerError::GraphQL(errors)),
            _ => Ok(self.data),
        }
    }
}

/// A single GraphQL error from the response.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct GraphQLError {
    pub message: String,
}

impl std::fmt::Display for GraphQLError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

// ---------------------------------------------------------------------------
// Block
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Block {
    pub hash: String,
    pub height: i64,
    #[serde(default)]
    pub protocol_version: Option<i64>,
    #[serde(default)]
    pub timestamp: Option<i64>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub ledger_parameters: Option<String>,
}

// ---------------------------------------------------------------------------
// Query offset types (mirror GraphQL schema inputs)
// ---------------------------------------------------------------------------

/// Offset for block queries: fetch by height or hash.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum BlockOffset {
    Height { height: i64 },
    Hash { hash: String },
}

impl BlockOffset {
    pub fn height(h: i64) -> Self {
        Self::Height { height: h }
    }

    pub fn hash(h: impl Into<String>) -> Self {
        Self::Hash { hash: h.into() }
    }
}

// ---------------------------------------------------------------------------
// Query variable structs (used internally by IndexerClient)
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Default)]
pub(crate) struct BlockQueryVars {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<BlockOffset>,
}

// ---------------------------------------------------------------------------
// Query-specific response wrappers
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct BlockQueryData {
    pub block: Option<Block>,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_block() {
        let json = r#"{
            "hash": "abc123",
            "height": 42,
            "protocolVersion": 1,
            "timestamp": 1700000000,
            "author": "node-1",
            "ledgerParameters": null
        }"#;

        let block: Block = serde_json::from_str(json).unwrap();
        assert_eq!(block.hash, "abc123");
        assert_eq!(block.height, 42);
        assert_eq!(block.protocol_version, Some(1));
        assert_eq!(block.timestamp, Some(1700000000));
        assert_eq!(block.author, Some("node-1".to_string()));
    }

    #[test]
    fn graphql_errors_win_over_partial_data() {
        let partial = GraphQLResponse {
            data: Some(1),
            errors: Some(vec![GraphQLError {
                message: "boom".into(),
            }]),
        };
        assert!(matches!(
            partial.into_result(),
            Err(IndexerError::GraphQL(errors)) if errors[0].message == "boom"
        ));

        let no_errors = GraphQLResponse {
            data: Some(1),
            errors: Some(vec![]),
        };
        assert!(matches!(no_errors.into_result(), Ok(Some(1))));
    }
}
