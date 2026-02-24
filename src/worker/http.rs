use crate::domain::{Download, DownloadStatus};
use futures::stream::StreamExt;
use tokio::io::AsyncWriteExt;

pub struct HttpDownloadResult {
    pub download: Download,
    pub is_torrent_file: bool,
}

pub struct HttpDownloader {
    download: Download,
}

impl HttpDownloader {
    pub fn new(download: Download) -> Self {
        Self { download }
    }

    pub async fn run(&mut self) -> anyhow::Result<HttpDownloadResult> {
        let client = reqwest::Client::new();
        let response = client.get(&self.download.url).send().await?;

        // Detect torrent from content-type header
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_lowercase();

        let is_torrent_from_header = content_type.contains("application/x-bittorrent")
            || content_type.contains("application/octet-stream");

        // Extract filename from Content-Disposition if available
        if let Some(disposition) = response.headers().get("content-disposition") {
            if let Ok(val) = disposition.to_str() {
                if let Some(fname) = extract_filename_from_disposition(val) {
                    self.download.filename = fname.clone();
                    let dir = std::path::Path::new(&self.download.save_path)
                        .parent()
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_else(|| "/data".to_string());
                    self.download.save_path = format!("{}/{}", dir, fname);
                }
            }
        }

        let total_size: u64 = response
            .headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);

        self.download.total_size = total_size;

        let mut file = tokio::fs::File::create(&self.download.save_path).await?;
        let mut stream = response.bytes_stream();
        let mut downloaded: u64 = 0;
        let start_time = std::time::Instant::now();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            file.write_all(&chunk).await?;
            downloaded += chunk.len() as u64;

            self.download.downloaded_size = downloaded;
            if self.download.total_size > 0 {
                self.download.progress =
                    (downloaded as f64 / self.download.total_size as f64) * 100.0;
            }

            let elapsed = start_time.elapsed().as_secs_f64();
            if elapsed > 0.0 {
                self.download.speed = (downloaded as f64 / elapsed) as u64;
            }
        }

        file.flush().await?;
        self.download.status = DownloadStatus::Completed;
        self.download.speed = 0;

        // Detect torrent by file extension or header
        let is_torrent = is_torrent_from_header
            || self.download.filename.ends_with(".torrent")
            || self.download.save_path.ends_with(".torrent");

        // Also check magic bytes if filename doesn't have extension
        let is_torrent = if !is_torrent {
            check_torrent_magic(&self.download.save_path).await
        } else {
            true
        };

        Ok(HttpDownloadResult {
            download: self.download.clone(),
            is_torrent_file: is_torrent,
        })
    }
}

fn extract_filename_from_disposition(header: &str) -> Option<String> {
    // Parse: attachment; filename="file.torrent" or filename=file.torrent
    for part in header.split(';') {
        let part = part.trim();
        if let Some(rest) = part.strip_prefix("filename=") {
            let name = rest.trim_matches('"').trim_matches('\'');
            if !name.is_empty() {
                return Some(name.to_string());
            }
        } else if let Some(rest) = part.strip_prefix("filename*=") {
            // Handle RFC 5987 encoded filenames: UTF-8''filename.torrent
            if let Some(fname) = rest.split("''").nth(1) {
                let decoded = urlencoding_decode(fname);
                if !decoded.is_empty() {
                    return Some(decoded);
                }
            }
        }
    }
    None
}

fn urlencoding_decode(s: &str) -> String {
    let mut result = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '%' {
            let hex: String = chars.by_ref().take(2).collect();
            if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                result.push(byte as char);
            }
        } else {
            result.push(c);
        }
    }
    result
}

async fn check_torrent_magic(path: &str) -> bool {
    // Torrent files start with "d" (bencode dictionary) followed by common keys
    // A more reliable check: starts with "d8:announce" or "d13:announce-list"
    if let Ok(bytes) = tokio::fs::read(path).await {
        if bytes.len() > 2 {
            return bytes[0] == b'd'
                && (bytes.starts_with(b"d8:announce")
                    || bytes.starts_with(b"d13:")
                    || bytes.starts_with(b"d7:comment")
                    || bytes.starts_with(b"d4:info"));
        }
    }
    false
}
