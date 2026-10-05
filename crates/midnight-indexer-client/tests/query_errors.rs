//! Mock-HTTP-server tests of `IndexerError::is_retryable` for the failures
//! of a one-shot indexer query.

use std::time::Duration;

use midnight_indexer_client::testutil::{bind, read_http_request, write_json_response};
use midnight_indexer_client::{IndexerClient, IndexerError};

#[tokio::test]
async fn a_query_is_retryable_after_a_dropped_connection_or_a_server_error() {
    // `None` drops the connection unanswered.
    let cases = [
        (None, true),
        (Some("503 Service Unavailable"), true),
        (Some("400 Bad Request"), false),
    ];
    for (status, retryable) in cases {
        let (listener, url) = bind().await;
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            if let Some(status) = status
                && read_http_request(&mut stream).await
            {
                write_json_response(&mut stream, status, "").await;
            }
        });
        let err = IndexerClient::new(&url)
            .unwrap()
            .get_block(None)
            .await
            .unwrap_err();
        assert_eq!(err.is_retryable(), retryable, "{status:?}: {err}");
    }
}

#[tokio::test]
async fn a_query_that_times_out_is_not_retryable() {
    let (listener, url) = bind().await;
    tokio::spawn(async move {
        let _held = listener.accept().await.unwrap();
        std::future::pending::<()>().await;
    });
    // `IndexerClient` has no way to shorten its timeout, so a plain client
    // with a short one makes the same reqwest error.
    let err = reqwest::Client::builder()
        .timeout(Duration::from_millis(100))
        .build()
        .unwrap()
        .post(format!("{url}/api/v3/graphql"))
        .send()
        .await
        .unwrap_err();
    assert!(err.is_timeout(), "{err:?}");
    assert!(!IndexerError::from(err).is_retryable());
}
