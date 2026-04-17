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
            .redirect(crate::worker::ssrf_safe_redirect_policy())
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
        let mut writer = BufWriter::with_capacity(8 * 1024 * 1024, file);
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

        // Wait for all tasks — don't abort others if one fails
        let mut first_error: Option<anyhow::Error> = None;
        while let Some(result) = join_set.join_next().await {
            match result {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    if first_error.is_none() {
                        first_error = Some(e);
                    }
                }
                Err(e) => {
                    if first_error.is_none() {
                        first_error = Some(anyhow::anyhow!("Task panicked: {}", e));
                    }
                }
            }
        }
        self.active_conns.store(0, Ordering::Relaxed);

        match first_error {
            Some(e) => Err(e),
            None => Ok(()),
        }
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
    let max_retries = 5;
    let mut attempt = 0;
    let mut current_start = start;

    loop {
        attempt += 1;
        match download_range_inner(
            client,
            url,
            path,
            current_start,
            end,
            downloaded,
            cancel_token,
        )
        .await
        {
            Ok(()) => return Ok(()),
            Err((e, _)) if cancel_token.is_cancelled() => return Err(e),
            Err((e, bytes_written)) => {
                // Advance past successfully written bytes so retry resumes
                current_start += bytes_written;
                if current_start > end {
                    return Ok(()); // Already got everything despite the error
                }
                if attempt >= max_retries {
                    return Err(anyhow::anyhow!(
                        "Range {}-{} failed after {} attempts: {}",
                        start,
                        end,
                        max_retries,
                        e
                    ));
                }
                let backoff = Duration::from_secs(2u64.pow(attempt as u32 - 1).min(30));
                tracing::warn!(
                    "Range {}-{} attempt {}/{} failed at byte {}: {}, retrying in {:?}...",
                    start,
                    end,
                    attempt,
                    max_retries,
                    current_start,
                    e,
                    backoff,
                );
                tokio::time::sleep(backoff).await;
            }
        }
    }
}

/// Returns Ok(()) on success, or Err((error, bytes_successfully_written)) on failure.
/// Bytes written are NOT subtracted from `downloaded` — the caller handles resume logic.
async fn download_range_inner(
    client: &reqwest::Client,
    url: &str,
    path: &str,
    start: u64,
    end: u64,
    downloaded: &Arc<AtomicU64>,
    cancel_token: &CancellationToken,
) -> Result<(), (anyhow::Error, u64)> {
    let resp = client
        .get(url)
        .header("Range", format!("bytes={}-{}", start, end))
        .send()
        .await
        .map_err(|e| (e.into(), 0u64))?;

    let status = resp.status();
    if status == reqwest::StatusCode::OK && start > 0 {
        return Err((
            anyhow::anyhow!(
                "Server ignored Range header (returned 200 instead of 206 for bytes {}-{})",
                start,
                end
            ),
            0,
        ));
    }
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS
        || status == reqwest::StatusCode::SERVICE_UNAVAILABLE
    {
        return Err((
            anyhow::anyhow!("Server throttling: {} for range {}-{}", status, start, end),
            0,
        ));
    }
    if status != reqwest::StatusCode::PARTIAL_CONTENT && status != reqwest::StatusCode::OK {
        return Err((
            anyhow::anyhow!("Unexpected status {} for range request", status),
            0,
        ));
    }

    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .await
        .map_err(|e| (e.into(), 0u64))?;
    file.seek(std::io::SeekFrom::Start(start))
        .await
        .map_err(|e| (e.into(), 0u64))?;
    let mut writer = BufWriter::with_capacity(16 * 1024 * 1024, file);

    let expected = match (end - start).checked_add(1) {
        Some(v) => v,
        None => {
            return Err((
                anyhow::anyhow!("Range length overflow: end={}, start={}", end, start),
                0u64,
            ));
        }
    };
    let mut bytes_written: u64 = 0;
    let mut stream = resp.bytes_stream();

    let loop_result: Result<(), anyhow::Error> = loop {
        let chunk_result = tokio::time::timeout(Duration::from_secs(60), stream.next()).await;
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
                    "Stalled for 60s after {} of {} bytes",
                    bytes_written,
                    expected
                ));
            }
        }
    };

    if let Err(e) = loop_result {
        // Flush what we have — caller will resume from bytes_written offset
        let _ = writer.flush().await;
        return Err((e, bytes_written));
    }

    if let Err(e) = writer.flush().await {
        return Err((e.into(), bytes_written));
    }

    // Allow slight overshoot (some servers send extra bytes) but catch short reads
    if bytes_written < expected {
        return Err((
            anyhow::anyhow!(
                "Range {}-{}: expected {} bytes, got {}",
                start,
                end,
                expected,
                bytes_written
            ),
            bytes_written,
        ));
    }

    Ok(())
}

fn extract_filename_from_disposition(header: &str) -> Option<String> {
    let parsed = content_disposition::parse_content_disposition(header);

    // The content_disposition crate decodes `filename*` into the "filename" key only
    // when no plain "filename" key exists. When both are present `filename*` stays
    // raw — decode it here so RFC 6266 §4.1 precedence is honored.
    if let Some(raw_star) = parsed.params.get("filename*") {
        let parts: Vec<&str> = raw_star.splitn(3, '\'').collect();
        if parts.len() == 3 {
            let charset = parts[0].to_uppercase();
            if charset == "UTF-8" || charset.is_empty() {
                let decoded = rfc5987_percent_decode(parts[2]);
                let s = String::from_utf8_lossy(&decoded).into_owned();
                if !s.is_empty() {
                    return Some(s);
                }
            }
            // Non-UTF-8 charsets fall through to the plain filename below.
        }
    }

    parsed
        .params
        .get("filename")
        .map(|name| unescape_quoted_pairs(name))
        .filter(|s| !s.is_empty())
}

fn rfc5987_percent_decode(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = bytes[i + 1];
            let lo = bytes[i + 2];
            if hi.is_ascii_hexdigit() && lo.is_ascii_hexdigit() {
                out.push((hex_nibble(hi) << 4) | hex_nibble(lo));
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

fn hex_nibble(b: u8) -> u8 {
    match b {
        b'0'..=b'9' => b - b'0',
        b'a'..=b'f' => b - b'a' + 10,
        b'A'..=b'F' => b - b'A' + 10,
        _ => 0,
    }
}

fn unescape_quoted_pairs(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(&next) = chars.peek() {
                out.push(next);
                chars.next();
                continue;
            }
        }
        out.push(c);
    }
    out
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

#[cfg(test)]
mod tests {
    use super::*;

    // ─── rfc5987_percent_decode ──────────────────────────────────────────

    #[test]
    fn percent_decode_basic() {
        assert_eq!(rfc5987_percent_decode("foo%20bar"), b"foo bar");
        assert_eq!(rfc5987_percent_decode("f%C3%B6o"), "föo".as_bytes());
    }

    #[test]
    fn percent_decode_leaves_plain_chars() {
        assert_eq!(rfc5987_percent_decode("plain.txt"), b"plain.txt");
    }

    #[test]
    fn percent_decode_invalid_sequence_kept_literal() {
        // %ZZ is not valid hex — should pass through bytes as-is
        assert_eq!(rfc5987_percent_decode("x%ZZy"), b"x%ZZy");
    }

    #[test]
    fn percent_decode_trailing_percent_kept() {
        assert_eq!(rfc5987_percent_decode("x%"), b"x%");
        assert_eq!(rfc5987_percent_decode("x%A"), b"x%A");
    }

    #[test]
    fn percent_decode_mixed_case_hex() {
        assert_eq!(rfc5987_percent_decode("%Af"), &[0xaf]);
        assert_eq!(rfc5987_percent_decode("%aF"), &[0xaf]);
    }

    // ─── hex_nibble ──────────────────────────────────────────────────────

    #[test]
    fn hex_nibble_valid_digits() {
        for (i, c) in b"0123456789abcdef".iter().enumerate() {
            assert_eq!(hex_nibble(*c), i as u8, "lowercase {}", *c as char);
        }
        for (i, c) in b"0123456789ABCDEF".iter().enumerate() {
            assert_eq!(hex_nibble(*c), i as u8, "uppercase {}", *c as char);
        }
    }

    #[test]
    fn hex_nibble_invalid_is_zero() {
        // Documented behavior: non-hex falls through to 0, callers use
        // ascii_hexdigit guard.
        assert_eq!(hex_nibble(b'G'), 0);
        assert_eq!(hex_nibble(b' '), 0);
    }

    // ─── extract_filename_from_disposition ───────────────────────────────

    #[test]
    fn disposition_plain_filename() {
        assert_eq!(
            extract_filename_from_disposition(r#"attachment; filename="simple.zip""#),
            Some("simple.zip".to_string())
        );
    }

    #[test]
    fn disposition_filename_star_utf8_preferred_over_plain() {
        // RFC 6266 §4.1: filename* (if charset=UTF-8) takes precedence over filename
        let h = r#"attachment; filename="fallback.zip"; filename*=UTF-8''caf%C3%A9.zip"#;
        assert_eq!(
            extract_filename_from_disposition(h),
            Some("café.zip".to_string())
        );
    }

    #[test]
    fn disposition_non_utf8_charset_falls_back_to_plain() {
        let h = r#"attachment; filename="ascii.zip"; filename*=ISO-8859-1''caf%E9.zip"#;
        assert_eq!(
            extract_filename_from_disposition(h),
            Some("ascii.zip".to_string())
        );
    }

    #[test]
    fn disposition_unescapes_quoted_pairs_in_plain_filename() {
        // \" inside a quoted-string becomes "
        let h = r#"attachment; filename="one\"two.txt""#;
        assert_eq!(
            extract_filename_from_disposition(h),
            Some(r#"one"two.txt"#.to_string())
        );
    }

    #[test]
    fn disposition_none_when_header_has_no_filename() {
        assert_eq!(extract_filename_from_disposition("inline"), None);
        assert_eq!(extract_filename_from_disposition("attachment"), None);
    }

    #[test]
    fn disposition_empty_filename_rejected() {
        assert_eq!(
            extract_filename_from_disposition(r#"attachment; filename="""#),
            None
        );
    }
}
