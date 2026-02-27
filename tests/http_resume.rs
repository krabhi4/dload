use tempfile::TempDir;
use wiremock::{MockServer, Mock, ResponseTemplate};
use wiremock::matchers::{method, header_exists};

/// Returns `(content, expected_sha256_hex)` for a reproducible 512KB test payload.
fn test_payload() -> Vec<u8> {
    (0u8..=255).cycle().take(512 * 1024).collect()
}

#[tokio::test]
async fn resumes_from_existing_partial_file_when_range_supported() {
    let server = MockServer::start().await;
    let payload = test_payload();
    let total = payload.len() as u64;
    let partial_size: u64 = 256 * 1024; // 256KB already downloaded

    // HEAD: report size and range support
    Mock::given(method("HEAD"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-length", total.to_string().as_str())
                .insert_header("accept-ranges", "bytes"),
        )
        .mount(&server)
        .await;

    // Range request for the remaining bytes
    let remaining = payload[partial_size as usize..].to_vec();
    Mock::given(method("GET"))
        .and(header_exists("range"))
        .respond_with(
            ResponseTemplate::new(206)
                .set_body_bytes(remaining.clone())
                .insert_header("content-range", format!("bytes {}-{}/{}", partial_size, total - 1, total).as_str())
                .insert_header("content-length", remaining.len().to_string().as_str()),
        )
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let file_path = dir.path().join("testfile.bin");

    // Pre-create a partial file
    tokio::fs::write(&file_path, &payload[..partial_size as usize]).await.unwrap();

    let url = format!("{}/testfile.bin", server.uri());
    let mut download = dload::domain::Download::new(url, dir.path().to_str().unwrap());
    download.save_path = file_path.to_str().unwrap().to_string();

    let cancel = tokio_util::sync::CancellationToken::new();
    let mut downloader = dload::worker::http::HttpDownloader::new(download, 1, cancel);
    let result = downloader.run().await.unwrap();

    // File should be complete and match the original payload
    let written = tokio::fs::read(&file_path).await.unwrap();
    assert_eq!(written.len() as u64, total, "final file should equal full payload");
    assert_eq!(written, payload, "file contents should match original payload");
    // downloaded_size reflects all bytes tracked (resume_offset + new bytes = total)
    assert_eq!(result.download.downloaded_size, total);
}
