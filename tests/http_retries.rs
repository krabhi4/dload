use tempfile::TempDir;
use wiremock::{MockServer, Mock, ResponseTemplate};
use wiremock::matchers::method;
use std::sync::atomic::AtomicU32;
use std::sync::Arc;

#[tokio::test]
async fn retries_transient_5xx_then_succeeds() {
    let server = MockServer::start().await;
    let payload: Vec<u8> = b"hello retry world".to_vec();
    let total = payload.len() as u64;

    let _call_count = Arc::new(AtomicU32::new(0));

    // HEAD succeeds immediately
    Mock::given(method("HEAD"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-length", total.to_string().as_str()),
        )
        .mount(&server)
        .await;

    // First GET: 503, second GET: 503, third GET: 200 with body
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(2)
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(payload.clone())
                .insert_header("content-length", total.to_string().as_str()),
        )
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let url = format!("{}/file.txt", server.uri());
    let download = dload::domain::Download::new(url, dir.path().to_str().unwrap());

    let cancel = tokio_util::sync::CancellationToken::new();
    let mut downloader = dload::worker::http::HttpDownloader::new(download, 1, cancel);
    let result = downloader.run().await;

    assert!(result.is_ok(), "should succeed after retries: {:?}", result.err());
    let file_path = result.unwrap().download.save_path;
    let written = tokio::fs::read(&file_path).await.unwrap();
    assert_eq!(written, payload);
}
