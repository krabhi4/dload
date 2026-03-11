use crate::domain::{sanitize_filename, Download, DownloadStatus};
use futures::stream::StreamExt;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncSeekExt, AsyncWriteExt, BufWriter};
use tokio_util::sync::CancellationToken;

pub struct HttpDownloadResult {
    pub download: Download,
    pub is_torrent_file: bool,
}

pub struct HttpDownloader {
    download: Download,
    max_connections: usize,
    min_split_size: u64,
    cancel_token: CancellationToken,
    // Shared progress — manager reads these from monitor loop
    pub downloaded: Arc<AtomicU64>,
    pub active_conns: Arc<AtomicU32>,
    pub total_size: Arc<AtomicU64>,
}

impl HttpDownloader {
    pub fn new(
        download: Download,
        max_connections: usize,
        min_split_size: u64,
        cancel_token: CancellationToken,
    ) -> Self {
        Self {
            download,
            max_connections,
            min_split_size,
            cancel_token,
            downloaded: Arc::new(AtomicU64::new(0)),
            active_conns: Arc::new(AtomicU32::new(0)),
            total_size: Arc::new(AtomicU64::new(0)),
        }
    }

    pub async fn run(&mut self) -> anyhow::Result<HttpDownloadResult> {
        let client = reqwest::Client::builder()
            .tcp_nodelay(true)
            .pool_max_idle_per_host(self.max_connections + 2)
            .connect_timeout(Duration::from_secs(30))
            .read_timeout(Duration::from_secs(30))
            .user_agent(format!("DLoad/{}", env!("CARGO_PKG_VERSION")))
            .build()?;

        // HEAD request to get content-length and check range support
        let head_resp = client.head(&self.download.url).send().await?;
        let headers = head_resp.headers().clone();

        let content_length: u64 = headers
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);

        let accepts_ranges = headers
            .get("accept-ranges")
            .and_then(|v| v.to_str().ok())
            .map(|v| v.contains("bytes"))
            .unwrap_or(false);

        // Extract filename from Content-Disposition
        if let Some(disposition) = headers.get("content-disposition") {
            if let Ok(val) = disposition.to_str() {
                if let Some(raw_fname) = extract_filename_from_disposition(val) {
                    let fname = sanitize_filename(&raw_fname);
                    self.download.filename = fname.clone();
                    let dir = std::path::Path::new(&self.download.save_path)
                        .parent()
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_else(|| "/data".to_string());
                    self.download.save_path = format!("{}/{}", dir, fname);
                }
            }
        }

        // Detect torrent from content-type
        let content_type = headers
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_lowercase();

        let is_torrent_from_header = content_type.contains("application/x-bittorrent");

        self.download.total_size = content_length;
        self.total_size.store(content_length, Ordering::Relaxed);

        // Decide number of connections
        let min_chunk = self.min_split_size.max(1024 * 1024); // floor at 1MB
        let num_conns = if accepts_ranges && content_length > min_chunk {
            let possible = (content_length / min_chunk) as usize;
            possible.min(self.max_connections).max(1)
        } else {
            1
        };

        if num_conns <= 1 || content_length == 0 {
            // Single-connection download
            self.download_single(&client).await?;
        } else {
            // Multi-connection download
            self.download_multi(&client, content_length, num_conns)
                .await?;
        }

        self.download.status = DownloadStatus::Completed;
        self.download.speed = 0;
        self.download.downloaded_size = self.downloaded.load(Ordering::Relaxed);

        // Detect torrent file
        let is_torrent = is_torrent_from_header
            || self.download.filename.ends_with(".torrent")
            || self.download.save_path.ends_with(".torrent");

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

    async fn download_single(&mut self, client: &reqwest::Client) -> anyhow::Result<()> {
        let response = client.get(&self.download.url).send().await?;

        // Update filename/size from GET response if HEAD didn't provide them
        if self.download.total_size == 0 {
            if let Some(len) = response.headers().get("content-length") {
                if let Ok(s) = len.to_str() {
                    if let Ok(v) = s.parse::<u64>() {
                        self.download.total_size = v;
                        self.total_size.store(v, Ordering::Relaxed);
                    }
                }
            }
        }
        if let Some(disposition) = response.headers().get("content-disposition") {
            if let Ok(val) = disposition.to_str() {
                if let Some(raw_fname) = extract_filename_from_disposition(val) {
                    let fname = sanitize_filename(&raw_fname);
                    self.download.filename = fname.clone();
                    let dir = std::path::Path::new(&self.download.save_path)
                        .parent()
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_else(|| "/data".to_string());
                    self.download.save_path = format!("{}/{}", dir, fname);
                }
            }
        }

        self.active_conns.store(1, Ordering::Relaxed);
        let file = tokio::fs::File::create(&self.download.save_path).await?;
        let mut writer = BufWriter::with_capacity(1024 * 1024, file);
        let mut stream = response.bytes_stream();

        while let Some(chunk) = stream.next().await {
            if self.cancel_token.is_cancelled() {
                let _ = writer.flush().await;
                self.active_conns.store(0, Ordering::Relaxed);
                return Err(anyhow::anyhow!("Download cancelled"));
            }
            let chunk = chunk?;
            writer.write_all(&chunk).await?;
            self.downloaded
                .fetch_add(chunk.len() as u64, Ordering::Relaxed);
        }

        writer.flush().await?;
        self.active_conns.store(0, Ordering::Relaxed);
        Ok(())
    }

    async fn download_multi(
        &mut self,
        client: &reqwest::Client,
        total_size: u64,
        num_conns: usize,
    ) -> anyhow::Result<()> {
        // Pre-allocate file
        let file = tokio::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&self.download.save_path)
            .await?;
        file.set_len(total_size).await?;
        drop(file);

        let chunk_size = total_size / num_conns as u64;
        let mut join_set = tokio::task::JoinSet::new();

        for i in 0..num_conns {
            let start = i as u64 * chunk_size;
            let end = if i == num_conns - 1 {
                total_size - 1
            } else {
                start + chunk_size - 1
            };

            let client = client.clone();
            let url = self.download.url.clone();
            let path = self.download.save_path.clone();
            let downloaded = Arc::clone(&self.downloaded);
            let active_conns = Arc::clone(&self.active_conns);
            let cancel_token = self.cancel_token.clone();

            join_set.spawn(async move {
                active_conns.fetch_add(1, Ordering::Relaxed);
                let result =
                    download_range(&client, &url, &path, start, end, &downloaded, &cancel_token)
                        .await;
                active_conns.fetch_sub(1, Ordering::Relaxed);
                result
            });
        }

        // Wait for all tasks
        while let Some(result) = join_set.join_next().await {
            match result {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    join_set.abort_all();
                    self.active_conns.store(0, Ordering::Relaxed);
                    return Err(e);
                }
                Err(e) => {
                    join_set.abort_all();
                    self.active_conns.store(0, Ordering::Relaxed);
                    return Err(anyhow::anyhow!("Task panicked: {}", e));
                }
            }
        }

        Ok(())
    }
}

async fn download_range(
    client: &reqwest::Client,
    url: &str,
    path: &str,
    start: u64,
    end: u64,
    downloaded: &Arc<AtomicU64>,
    cancel_token: &CancellationToken,
) -> anyhow::Result<()> {
    let max_retries = 3;
    let mut attempt = 0;

    loop {
        attempt += 1;
        match download_range_inner(client, url, path, start, end, downloaded, cancel_token).await {
            Ok(()) => return Ok(()),
            Err(e) if cancel_token.is_cancelled() => return Err(e),
            Err(e) if attempt >= max_retries => {
                return Err(anyhow::anyhow!(
                    "Range {}-{} failed after {} attempts: {}",
                    start,
                    end,
                    max_retries,
                    e
                ));
            }
            Err(e) => {
                tracing::warn!(
                    "Range {}-{} attempt {}/{} failed: {}, retrying...",
                    start,
                    end,
                    attempt,
                    max_retries,
                    e
                );
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
    }
}

async fn download_range_inner(
    client: &reqwest::Client,
    url: &str,
    path: &str,
    start: u64,
    end: u64,
    downloaded: &Arc<AtomicU64>,
    cancel_token: &CancellationToken,
) -> anyhow::Result<()> {
    let resp = client
        .get(url)
        .header("Range", format!("bytes={}-{}", start, end))
        .send()
        .await?;

    let status = resp.status();
    if status == reqwest::StatusCode::OK && start > 0 {
        return Err(anyhow::anyhow!(
            "Server ignored Range header (returned 200 instead of 206 for bytes {}-{})",
            start,
            end
        ));
    }
    if status != reqwest::StatusCode::PARTIAL_CONTENT && status != reqwest::StatusCode::OK {
        return Err(anyhow::anyhow!(
            "Unexpected status {} for range request",
            status
        ));
    }

    let mut file = tokio::fs::OpenOptions::new().write(true).open(path).await?;
    file.seek(std::io::SeekFrom::Start(start)).await?;
    let mut writer = BufWriter::with_capacity(1024 * 1024, file);

    let expected = end - start + 1;
    let mut bytes_written: u64 = 0;
    let mut stream = resp.bytes_stream();

    // Use break-with-value so ALL error paths go through a single cleanup point
    let loop_result: anyhow::Result<()> = loop {
        let chunk_result = tokio::time::timeout(Duration::from_secs(15), stream.next()).await;
        match chunk_result {
            Ok(Some(chunk)) => {
                if cancel_token.is_cancelled() {
                    let _ = writer.flush().await;
                    break Err(anyhow::anyhow!("Download cancelled"));
                }
                let chunk = match chunk {
                    Ok(c) => c,
                    Err(e) => break Err(e.into()),
                };
                if let Err(e) = writer.write_all(&chunk).await {
                    break Err(e.into());
                }
                let len = chunk.len() as u64;
                bytes_written += len;
                downloaded.fetch_add(len, Ordering::Relaxed);
            }
            Ok(None) => break Ok(()), // Stream ended
            Err(_) => {
                let _ = writer.flush().await;
                break Err(anyhow::anyhow!(
                    "Range {}-{}: stalled for 15s after {} bytes",
                    start,
                    end,
                    bytes_written
                ));
            }
        }
    };

    if let Err(e) = loop_result {
        downloaded.fetch_sub(bytes_written, Ordering::Relaxed);
        return Err(e);
    }

    if let Err(e) = writer.flush().await {
        downloaded.fetch_sub(bytes_written, Ordering::Relaxed);
        return Err(e.into());
    }

    if bytes_written != expected {
        downloaded.fetch_sub(bytes_written, Ordering::Relaxed);
        return Err(anyhow::anyhow!(
            "Range {}-{}: expected {} bytes, got {}",
            start,
            end,
            expected,
            bytes_written
        ));
    }

    Ok(())
}

fn extract_filename_from_disposition(header: &str) -> Option<String> {
    for part in header.split(';') {
        let part = part.trim();
        if let Some(rest) = part.strip_prefix("filename=") {
            let name = rest.trim_matches('"').trim_matches('\'');
            if !name.is_empty() {
                return Some(name.to_string());
            }
        } else if let Some(rest) = part.strip_prefix("filename*=") {
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
