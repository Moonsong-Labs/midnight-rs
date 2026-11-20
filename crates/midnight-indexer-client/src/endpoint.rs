use reqwest::header::{HeaderMap, HeaderName, HeaderValue};

use crate::error::IndexerError;

/// The GraphQL path of a self-hosted indexer. The indexer serves `/api/v3`
/// as an alias of its current API version.
const GRAPHQL_PATH: &str = "/api/v3/graphql";

/// An indexer to query and subscribe to: its URL, and the credential that it
/// requires, if any.
///
/// The URL gives the GraphQL endpoint of the queries:
///
/// - A URL with no path, such as `http://127.0.0.1:8088`, is a self-hosted
///   indexer. The client adds `/api/v3/graphql`.
/// - A URL whose path ends in `/graphql/ws` is the subscription endpoint of
///   a self-hosted indexer. The client removes `/ws`.
/// - The client uses any other URL as given, such as
///   `https://midnight-preprod.blockfrost.io/api/v0`.
///
/// The subscriptions use the GraphQL endpoint followed by `/ws`, with `ws` in
/// place of `http` and `wss` in place of `https`.
///
/// A string converts into an endpoint with no credential, so a signature
/// that takes `impl Into<IndexerEndpoint>` also takes a URL.
///
/// # Example
///
/// Blockfrost hosts the indexer of PreProd and mainnet, and requires a
/// project token on each request:
///
/// ```
/// use midnight_indexer_client::IndexerEndpoint;
///
/// # fn main() -> Result<(), midnight_indexer_client::IndexerError> {
/// # let token = "preprodToken";
/// let indexer = IndexerEndpoint::new("https://midnight-preprod.blockfrost.io/api/v0")
///     .with_credential("project_id", token)?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct IndexerEndpoint {
    url: String,
    /// Each value is sensitive, so `Debug` prints `Sensitive` for it.
    headers: HeaderMap,
}

impl IndexerEndpoint {
    /// An endpoint at `url` with no credential.
    ///
    /// A URL with a query fails
    /// [`IndexerClient::new`](crate::IndexerClient::new) and each subscription
    /// with [`IndexerError::Config`]. Send a token with
    /// [`Self::with_credential`].
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            headers: HeaderMap::new(),
        }
    }

    /// Send `value` in the request header `header` on each query and on the
    /// WebSocket upgrade of each subscription. Blockfrost reads its project
    /// token from the header `project_id`. A second call with the same
    /// header replaces the value.
    ///
    /// Put a token here, not in the URL: the URL goes into logs and error
    /// messages. `Debug` prints `Sensitive` in place of the value.
    ///
    /// # Errors
    ///
    /// [`IndexerError::Config`] when `header` is not a valid header name, or
    /// when `value` holds a character that is not visible ASCII, such as a
    /// line break or a non-breaking space. The message holds neither
    /// argument.
    pub fn with_credential(mut self, header: &str, value: &str) -> Result<Self, IndexerError> {
        let name = HeaderName::try_from(header).map_err(|e| {
            IndexerError::Config(format!("the credential header name is not valid: {e}"))
        })?;
        let mut value = HeaderValue::from_str(value)
            .map_err(|e| IndexerError::Config(format!("the credential of {name}: {e}")))?;
        // The WebSocket handshake writes each header with `to_str`, which
        // refuses bytes above 0x7f that `from_str` accepts.
        value
            .to_str()
            .map_err(|e| IndexerError::Config(format!("the credential of {name}: {e}")))?;
        value.set_sensitive(true);
        self.headers.insert(name, value);
        Ok(self)
    }

    /// The URL of the GraphQL queries.
    pub(crate) fn graphql_url(&self) -> Result<String, IndexerError> {
        // Refuse before the layout: a path added after the query would land
        // inside it.
        if self.url.contains('?') {
            return Err(IndexerError::Config(
                "the indexer URL has a query: send a token with IndexerEndpoint::with_credential"
                    .into(),
            ));
        }
        let url = self.url.trim_end_matches('/');
        let url = url
            .strip_suffix("/ws")
            .filter(|graphql| graphql.ends_with("/graphql"))
            .unwrap_or(url);
        let authority_and_path = url.split_once("://").map_or(url, |(_, rest)| rest);
        if authority_and_path.contains('/') {
            Ok(url.to_owned())
        } else {
            Ok(format!("{url}{GRAPHQL_PATH}"))
        }
    }

    /// The URL of the subscriptions.
    pub(crate) fn ws_url(&self) -> Result<String, IndexerError> {
        let url = format!("{}/ws", self.graphql_url()?);
        Ok(if let Some(rest) = url.strip_prefix("http://") {
            format!("ws://{rest}")
        } else if let Some(rest) = url.strip_prefix("https://") {
            format!("wss://{rest}")
        } else {
            url
        })
    }

    /// The headers of each request.
    pub(crate) fn headers(&self) -> &HeaderMap {
        &self.headers
    }
}

impl From<&str> for IndexerEndpoint {
    fn from(url: &str) -> Self {
        Self::new(url)
    }
}

impl From<String> for IndexerEndpoint {
    fn from(url: String) -> Self {
        Self::new(url)
    }
}

impl From<&String> for IndexerEndpoint {
    fn from(url: &String) -> Self {
        Self::new(url.as_str())
    }
}

impl From<&IndexerEndpoint> for IndexerEndpoint {
    fn from(endpoint: &IndexerEndpoint) -> Self {
        endpoint.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "preprodSECRET";

    /// It catches a rule that adds the default path to a URL with a path,
    /// such as the Blockfrost `/api/v0`, a rule that adds nothing to a bare
    /// host, and a subscription URL with a doubled `/ws` or an `http` scheme.
    #[test]
    fn the_url_gives_the_query_and_subscription_urls() {
        for (url, graphql, ws) in [
            (
                "http://127.0.0.1:8088",
                "http://127.0.0.1:8088/api/v3/graphql",
                "ws://127.0.0.1:8088/api/v3/graphql/ws",
            ),
            (
                "https://indexer.example/",
                "https://indexer.example/api/v3/graphql",
                "wss://indexer.example/api/v3/graphql/ws",
            ),
            (
                "http://127.0.0.1:8088/api/v3/graphql",
                "http://127.0.0.1:8088/api/v3/graphql",
                "ws://127.0.0.1:8088/api/v3/graphql/ws",
            ),
            (
                "http://127.0.0.1:8088/api/v3/graphql/ws",
                "http://127.0.0.1:8088/api/v3/graphql",
                "ws://127.0.0.1:8088/api/v3/graphql/ws",
            ),
            (
                "https://midnight-preprod.blockfrost.io/api/v0/",
                "https://midnight-preprod.blockfrost.io/api/v0",
                "wss://midnight-preprod.blockfrost.io/api/v0/ws",
            ),
        ] {
            let endpoint = IndexerEndpoint::new(url);
            assert_eq!(
                (endpoint.graphql_url().unwrap(), endpoint.ws_url().unwrap()),
                (graphql.to_owned(), ws.to_owned()),
                "{url}"
            );
        }
    }

    /// It catches a `Debug` that prints the value, for example from a value
    /// that is not marked sensitive.
    #[test]
    fn debug_hides_the_credential() {
        let endpoint = IndexerEndpoint::new("https://indexer.example")
            .with_credential("project_id", SECRET)
            .unwrap();
        let debug = format!("{endpoint:?}");
        assert!(!debug.contains(SECRET), "{debug}");
    }

    /// It catches a check with `HeaderValue::from_str` alone, which accepts
    /// the bytes of a non-breaking space. The WebSocket handshake then
    /// refuses the value, while the queries send it.
    #[test]
    fn a_credential_that_is_not_visible_ascii_is_refused() {
        let value = format!("{SECRET}\u{a0}");
        let err = IndexerEndpoint::new("https://indexer.example")
            .with_credential("project_id", &value)
            .unwrap_err();
        assert!(
            matches!(&err, IndexerError::Config(message) if !message.contains(SECRET)),
            "{err:?}"
        );
    }
}
