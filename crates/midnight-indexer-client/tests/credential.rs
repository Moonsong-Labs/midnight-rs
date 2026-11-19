//! What a credential endpoint sends, as the server reads it: the path as
//! given, and the credential as a request header, on a query and on the
//! WebSocket upgrade of a subscription.

use std::time::Duration;

use midnight_indexer_client::testutil::{bind, read_http_request, write_json_response};
use midnight_indexer_client::{IndexerClient, IndexerEndpoint, IndexerError, SubscriptionClient};

const SECRET: &str = "preprodSECRET";

/// An endpoint in the Blockfrost layout at the mock server `url`.
fn blockfrost_layout(url: &str) -> IndexerEndpoint {
    IndexerEndpoint::new(format!("{url}/api/v0"))
        .with_credential("project_id", SECRET)
        .unwrap()
}

/// The request line and the value of the header `name` in the request
/// head `head`.
fn request_line_and_header<'a>(head: &'a str, name: &str) -> (&'a str, Option<&'a str>) {
    let mut lines = head.lines();
    let request_line = lines.next().unwrap_or_default();
    let value = lines.find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.trim().eq_ignore_ascii_case(name).then(|| value.trim())
    });
    (request_line, value)
}

/// It catches a client that does not send the credential on a query, which
/// Blockfrost answers with 403, and a client that changes the path.
#[tokio::test]
async fn a_query_sends_the_credential_header() {
    let (listener, url) = bind().await;
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let head = read_http_request(&mut stream).await.expect("a request");
        write_json_response(&mut stream, "200 OK", r#"{"data":{"block":null}}"#).await;
        head
    });

    IndexerClient::new(blockfrost_layout(&url))
        .unwrap()
        .get_block(None)
        .await
        .expect("the query");

    let head = server.await.unwrap();
    assert_eq!(
        request_line_and_header(&head, "project_id"),
        ("POST /api/v0 HTTP/1.1", Some(SECRET)),
        "{head}"
    );
}

/// It catches a client that sends the credential on the queries but not on
/// the WebSocket upgrade. The queries of a sync then work, and each
/// subscription fails.
#[tokio::test]
async fn a_subscription_sends_the_credential_on_the_upgrade() {
    let (listener, url) = bind().await;
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_http_request(&mut stream).await.expect("an upgrade")
    });

    // The server closes the connection after it reads the upgrade, so the
    // subscription fails.
    let _ = SubscriptionClient::new(blockfrost_layout(&url))
        .with_connect_timeout(Duration::from_secs(5))
        .subscribe::<serde_json::Value>("subscription { events }", serde_json::json!({}))
        .await;

    let head = server.await.unwrap();
    assert_eq!(
        request_line_and_header(&head, "project_id"),
        ("GET /api/v0/ws HTTP/1.1", Some(SECRET)),
        "{head}"
    );
}

/// It catches a refusal that is missing from one client, an error that
/// repeats the URL and its token, and a refusal that a caller can retry.
#[tokio::test]
async fn a_url_with_a_query_is_refused() {
    let url = format!("https://indexer.example/api/v0?project_id={SECRET}");
    let refused = |err: IndexerError| {
        assert!(
            !err.is_retryable()
                && matches!(&err, IndexerError::Config(message)
                    if message.contains("with_credential") && !message.contains(SECRET)),
            "{err:?}"
        );
    };

    refused(IndexerClient::new(url.as_str()).err().expect("a refusal"));
    refused(
        SubscriptionClient::new(url.as_str())
            .subscribe::<serde_json::Value>("subscription { events }", serde_json::json!({}))
            .await
            .expect_err("a refusal"),
    );
}
