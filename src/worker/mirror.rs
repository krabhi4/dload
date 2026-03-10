use futures::stream::StreamExt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncSeekExt, AsyncWriteExt, BufWriter};
use tokio_util::sync::CancellationToken;

/// Downloads a file via HTTP to a fixed path without mutating any Download fields.
#[allow(dead_code)]
pub struct MirrorDownloader {
    url: String,
    target_path: String,
    cancel_token: CancellationToken,
    pub downloaded: Arc<AtomicU64>,
    pub total_size: Arc<AtomicU64>,
}

#[allow(dead_code)]
pub struct MirrorResult {
    pub is_zip: bool,
    pub bytes_downloaded: u64,
}

#[allow(dead_code)]
impl MirrorDownloader {
    pub fn new(url: String, target_path: String, cancel_token: CancellationToken) -> Self {
        Self {
            url,
            target_path,
            cancel_token,
            downloaded: Arc::new(AtomicU64::new(0)),
            total_size: Arc::new(AtomicU64::new(0)),
        }
    }

    pub async fn run(&self) -> anyhow::Result<MirrorResult> {
        let client = reqwest::Client::builder()
            .tcp_nodelay(true)
            .pool_max_idle_per_host(6)
            .connect_timeout(Duration::from_secs(30))
            .read_timeout(Duration::from_secs(60))
            .build()?;

        // HEAD to get size and range support
        let head_resp = client.head(&self.url).send().await?;
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

        // Initial zip detection from HEAD (will be refined from GET in single-connection path)
        let head_content_type = headers
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_lowercase();
        let mut is_zip = head_content_type.contains("application/zip")
            || self.url.to_lowercase().ends_with(".zip");

        self.total_size.store(content_length, Ordering::Relaxed);

        // Decide connections
        let min_chunk: u64 = 2 * 1024 * 1024; // 2MB minimum per chunk
        let max_conns = 4usize;
        let num_conns = if accepts_ranges && content_length > min_chunk {
            let possible = (content_length / min_chunk) as usize;
            possible.min(max_conns).max(1)
        } else {
            1
        };

        if num_conns <= 1 || content_length == 0 {
            // Single-connection: refine zip detection from GET Content-Type
            let get_resp = client.get(&self.url).send().await?;
            let get_ct = get_resp
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_lowercase();
            if get_ct.contains("application/zip") {
                is_zip = true;
            }
            self.download_single_from_response(get_resp).await?;
        } else {
            self.download_multi(&client, content_length, num_conns).await?;
        }

        Ok(MirrorResult {
            is_zip,
            bytes_downloaded: self.downloaded.load(Ordering::Relaxed),
        })
    }

    async fn download_single_from_response(&self, response: reqwest::Response) -> anyhow::Result<()> {
        // Update total_size from GET if HEAD didn't provide it
        if self.total_size.load(Ordering::Relaxed) == 0 {
            if let Some(len) = response.headers().get("content-length") {
                if let Ok(v) = len.to_str().unwrap_or("0").parse::<u64>() {
                    self.total_size.store(v, Ordering::Relaxed);
                }
            }
        }

        let file = tokio::fs::File::create(&self.target_path).await?;
        let mut writer = BufWriter::with_capacity(256 * 1024, file);
        let mut stream = response.bytes_stream();

        while let Some(chunk) = stream.next().await {
            if self.cancel_token.is_cancelled() {
                let _ = writer.flush().await;
                return Err(anyhow::anyhow!("Mirror download cancelled"));
            }
            let chunk = chunk?;
            writer.write_all(&chunk).await?;
            self.downloaded
                .fetch_add(chunk.len() as u64, Ordering::Relaxed);
        }

        writer.flush().await?;
        Ok(())
    }

    async fn download_multi(
        &self,
        client: &reqwest::Client,
        total_size: u64,
        num_conns: usize,
    ) -> anyhow::Result<()> {
        // Pre-allocate file
        let file = tokio::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&self.target_path)
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
            let url = self.url.clone();
            let path = self.target_path.clone();
            let downloaded = Arc::clone(&self.downloaded);
            let cancel_token = self.cancel_token.clone();

            join_set.spawn(async move {
                mirror_download_range(&client, &url, &path, start, end, &downloaded, &cancel_token)
                    .await
            });
        }

        while let Some(result) = join_set.join_next().await {
            match result {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    join_set.abort_all();
                    return Err(e);
                }
                Err(e) => {
                    join_set.abort_all();
                    return Err(anyhow::anyhow!("Mirror download task panicked: {}", e));
                }
            }
        }

        Ok(())
    }
}

#[allow(dead_code)]
async fn mirror_download_range(
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
            "Server ignored Range header (returned 200 instead of 206)"
        ));
    }
    if status != reqwest::StatusCode::PARTIAL_CONTENT && status != reqwest::StatusCode::OK {
        return Err(anyhow::anyhow!("Unexpected status {} for range request", status));
    }

    let mut file = tokio::fs::OpenOptions::new().write(true).open(path).await?;
    file.seek(std::io::SeekFrom::Start(start)).await?;
    let mut writer = BufWriter::with_capacity(256 * 1024, file);

    let expected = end - start + 1;
    let mut bytes_written: u64 = 0;
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        if cancel_token.is_cancelled() {
            let _ = writer.flush().await;
            return Err(anyhow::anyhow!("Mirror download cancelled"));
        }
        let chunk = chunk?;
        writer.write_all(&chunk).await?;
        let len = chunk.len() as u64;
        bytes_written += len;
        downloaded.fetch_add(len, Ordering::Relaxed);
    }

    writer.flush().await?;

    if bytes_written != expected {
        return Err(anyhow::anyhow!(
            "Range {}-{}: expected {} bytes, got {}",
            start, end, expected, bytes_written
        ));
    }

    Ok(())
}

/// Extract a zip file to a target directory with path traversal protection.
/// Returns the list of extracted file paths (relative to target_dir).
#[allow(dead_code)]
pub fn extract_zip_safe(zip_path: &str, target_dir: &str) -> anyhow::Result<Vec<String>> {
    let file = std::fs::File::open(zip_path)?;
    let mut archive = zip::ZipArchive::new(file)?;
    let target = std::path::Path::new(target_dir);

    let mut extracted = Vec::new();

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let entry_path = match entry.enclosed_name() {
            Some(p) => p.to_owned(),
            None => {
                // enclosed_name() returns None for paths with .. or absolute paths
                tracing::warn!("Skipping unsafe zip entry: {:?}", entry.name());
                continue;
            }
        };

        let out_path = target.join(&entry_path);

        // Double-check the resolved path is within target
        let canonical_target = target.canonicalize().unwrap_or_else(|_| target.to_path_buf());
        if !out_path.starts_with(&canonical_target) {
            tracing::warn!("Zip entry escapes target dir: {:?}", entry_path);
            continue;
        }

        if entry.is_dir() {
            std::fs::create_dir_all(&out_path)?;
        } else {
            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut outfile = std::fs::File::create(&out_path)?;
            std::io::copy(&mut entry, &mut outfile)?;
            extracted.push(entry_path.to_string_lossy().to_string());
        }
    }

    Ok(extracted)
}
