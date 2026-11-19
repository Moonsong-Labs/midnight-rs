use std::time::Duration;

use serde::{Serialize, de::DeserializeOwned};
use tracing::{debug, warn};

use crate::endpoint::IndexerEndpoint;
use crate::error::IndexerError;
use crate::queries;
use crate::types::*;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// The most bytes of an undecodable response body that the warning logs.
const LOGGED_BODY_BYTES: usize = 512;

pub struct IndexerClient {
    http: reqwest::Client,
    http_url: String,
}

impl IndexerClient {
    /// Create a client that queries `endpoint`. [`IndexerEndpoint`] states
    /// the query URL, and the client sends the endpoint's credential on each
    /// query.
    ///
    /// # Errors
    ///
    /// [`IndexerError::Config`] when the endpoint URL has a query, or when
    /// the HTTP client cannot be built.
    pub fn new(endpoint: impl Into<IndexerEndpoint>) -> Result<Self, IndexerError> {
        let endpoint = endpoint.into();
        let http_url = endpoint.graphql_url()?;

        let http = reqwest::Client::builder()
            .timeout(DEFAULT_TIMEOUT)
            .default_headers(endpoint.headers().clone())
            .build()
            .map_err(|e| IndexerError::Config(e.to_string()))?;

        Ok(Self { http, http_url })
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
            let logged = &text[..text.floor_char_boundary(LOGGED_BODY_BYTES)];
            warn!(error = %e, body = logged, bytes = text.len(), "failed to deserialize indexer response");
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
