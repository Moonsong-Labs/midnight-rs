use std::time::Duration;

use serde::{Serialize, de::DeserializeOwned};
use tracing::{debug, warn};

use crate::error::IndexerError;
use crate::queries;
use crate::types::*;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

const GRAPHQL_PATH: &str = "/api/v3/graphql";

/// Returns the GraphQL endpoint for an indexer URL.
///
/// Keeps a URL whose path ends in `/graphql`, and drops a `/ws` that follows
/// `/graphql`. Appends [`GRAPHQL_PATH`] to any other URL.
pub(crate) fn graphql_url(base: &str) -> String {
    let base = base.trim_end_matches('/');
    let base = match base.strip_suffix("/ws") {
        Some(graphql) if graphql.ends_with("/graphql") => graphql,
        _ => base,
    };
    if base.ends_with("/graphql") {
        base.to_owned()
    } else {
        format!("{base}{GRAPHQL_PATH}")
    }
}

pub struct IndexerClient {
    http: reqwest::Client,
    http_url: String,
}

impl IndexerClient {
    /// Create a new client.
    ///
    /// `base_url` is the base indexer URL (e.g. `http://127.0.0.1:8088`), or
    /// a URL whose path ends in `/graphql` or `/graphql/ws`. The client sends
    /// queries to that `/graphql` path, or to `/api/v3/graphql` under a base
    /// URL. Returns an error if the HTTP client cannot be built.
    pub fn new(base_url: &str) -> Result<Self, IndexerError> {
        let http_url = graphql_url(base_url);

        let http = reqwest::Client::builder()
            .timeout(DEFAULT_TIMEOUT)
            .build()
            .map_err(|e| IndexerError::Config(e.to_string()))?;

        Ok(Self { http, http_url })
    }

    /// URL the client is connected to.
    pub fn url(&self) -> &str {
        &self.http_url
    }

    async fn execute<V: Serialize, R: DeserializeOwned>(
        &self,
        query: &str,
        variables: V,
    ) -> Result<R, IndexerError> {
        let body = serde_json::json!({
            "query": query,
            "variables": variables,
        });

        debug!(url = %self.http_url, "executing indexer GraphQL query");

        let resp = self
            .http
            .post(&self.http_url)
            .json(&body)
            .send()
            .await?
            .error_for_status()?;

        let text = resp.text().await?;

        let gql_resp: GraphQLResponse<R> = serde_json::from_str(&text).map_err(|e| {
            let truncated = if text.len() > 512 {
                format!("{}... ({} bytes)", &text[..512], text.len())
            } else {
                text.clone()
            };
            warn!(error = %e, body = %truncated, "failed to deserialize indexer response");
            IndexerError::Deserialization(e.to_string())
        })?;

        gql_resp.into_result()?.ok_or(IndexerError::MissingData)
    }

    // -- Blocks --

    /// Fetch a block by optional offset. Returns the latest block when
    /// `offset` is `None`.
    pub async fn get_block(
        &self,
        offset: Option<BlockOffset>,
    ) -> Result<Option<Block>, IndexerError> {
        let vars = BlockQueryVars { offset };
        let data: BlockQueryData = self.execute(queries::BLOCK_QUERY, vars).await?;
        Ok(data.block)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_construction_bare_host() {
        let client = IndexerClient::new("http://localhost:8088").unwrap();
        assert_eq!(client.url(), "http://localhost:8088/api/v3/graphql");
    }

    #[test]
    fn url_construction_with_trailing_slash() {
        let client = IndexerClient::new("http://localhost:8088/").unwrap();
        assert_eq!(client.url(), "http://localhost:8088/api/v3/graphql");
    }

    #[test]
    fn url_construction_full_path() {
        let client = IndexerClient::new("http://localhost:8088/api/v3/graphql").unwrap();
        assert_eq!(client.url(), "http://localhost:8088/api/v3/graphql");
    }

    #[test]
    fn graphql_url_drops_ws_only_after_graphql() {
        assert_eq!(
            graphql_url("http://localhost:8088/graphql/ws"),
            "http://localhost:8088/graphql"
        );
        assert_eq!(
            graphql_url("http://localhost:8088/foo/ws"),
            "http://localhost:8088/foo/ws/api/v3/graphql"
        );
    }
}
