use std::time::Duration;

use serde::{Serialize, de::DeserializeOwned};
use tracing::{debug, warn};

use crate::error::IndexerError;
use crate::queries;
use crate::types::*;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

pub struct IndexerClient {
    http: reqwest::Client,
    http_url: String,
}

impl IndexerClient {
    /// Create a new client. Appends `/api/v3/graphql` if not present.
    /// Returns an error if the HTTP client cannot be built.
    pub fn new(base_url: &str) -> Result<Self, IndexerError> {
        let base = base_url.trim_end_matches('/');

        let http_url = if base.ends_with("/api/v3/graphql") {
            base.to_string()
        } else {
            format!("{base}/api/v3/graphql")
        };

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

        if let Some(errors) = gql_resp.errors.filter(|e| !e.is_empty()) {
            return Err(IndexerError::GraphQL(errors));
        }

        gql_resp.data.ok_or(IndexerError::MissingData)
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

    /// Fetch a block with its transactions by optional offset.
    pub async fn get_block_with_transactions(
        &self,
        offset: Option<BlockOffset>,
    ) -> Result<Option<Block>, IndexerError> {
        let vars = BlockQueryVars { offset };
        let data: BlockQueryData = self
            .execute(queries::BLOCK_WITH_TRANSACTIONS_QUERY, vars)
            .await?;
        Ok(data.block)
    }

    // -- Contract State --

    /// Fetch raw hex-encoded contract state. Returns the latest state when
    /// `offset` is `None`.
    pub async fn get_contract_state(
        &self,
        address: &str,
        offset: Option<ContractActionOffset>,
    ) -> Result<Option<String>, IndexerError> {
        let vars = ContractActionQueryVars {
            address: address.to_string(),
            offset,
        };
        let data: ContractActionQueryData =
            self.execute(queries::CONTRACT_STATE_QUERY, vars).await?;
        Ok(data.contract_action.map(|a| a.state().to_string()))
    }

    // -- Contract Actions --

    /// Fetch a contract action (state + metadata). Returns the latest
    /// action when `offset` is `None`.
    pub async fn get_contract_action(
        &self,
        address: &str,
        offset: Option<ContractActionOffset>,
    ) -> Result<Option<ContractAction>, IndexerError> {
        let vars = ContractActionQueryVars {
            address: address.to_string(),
            offset,
        };
        let data: ContractActionQueryData =
            self.execute(queries::CONTRACT_ACTION_QUERY, vars).await?;
        Ok(data.contract_action)
    }

    /// Fetch the block height of the latest transaction touching a contract.
    pub async fn get_latest_contract_block_height(
        &self,
        address: &str,
    ) -> Result<Option<i64>, IndexerError> {
        #[derive(Debug, serde::Deserialize)]
        struct BlockHeight {
            height: i64,
        }
        #[derive(Debug, serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct TxWithBlock {
            block: Option<BlockHeight>,
        }
        #[derive(Debug, serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct ActionWithTx {
            transaction: Option<TxWithBlock>,
        }
        #[derive(Debug, serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Data {
            contract_action: Option<ActionWithTx>,
        }

        let vars = serde_json::json!({ "address": address });
        let data: Data = self
            .execute(queries::LATEST_CONTRACT_BLOCK_HEIGHT_QUERY, vars)
            .await?;

        Ok(data
            .contract_action
            .and_then(|a| a.transaction)
            .and_then(|t| t.block)
            .map(|b| b.height))
    }

    // -- Transactions --

    /// Fetch transactions by offset (hash or identifier).
    pub async fn get_transactions(
        &self,
        offset: TransactionOffset,
    ) -> Result<Vec<Transaction>, IndexerError> {
        let vars = TransactionsQueryVars { offset };
        let data: TransactionsQueryData = self.execute(queries::TRANSACTIONS_QUERY, vars).await?;
        Ok(data.transactions)
    }

    // -- Health --

    /// Returns true if the indexer is reachable and has blocks.
    pub async fn health_check(&self) -> bool {
        match self.get_block(None).await {
            Ok(Some(_)) => true,
            Ok(None) => {
                debug!("indexer returned no blocks (empty chain?)");
                true
            }
            Err(e) => {
                warn!(error = %e, "indexer health check failed");
                false
            }
        }
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
    fn url_construction_https() {
        let client = IndexerClient::new("https://indexer.midnight.network").unwrap();
        assert_eq!(
            client.url(),
            "https://indexer.midnight.network/api/v3/graphql"
        );
    }
}
