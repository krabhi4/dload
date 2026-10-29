use crate::domain::{sanitize_filename, Download, DownloadStatus};
use futures::stream::StreamExt;
use reqwest::header::{
    CONTENT_DISPOSITION, CONTENT_RANGE, CONTENT_TYPE, ETAG, LAST_MODIFIED, RANGE,
};
use reqwest::StatusCode;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::task::{JoinHandle, JoinSet};
use tokio_util::sync::CancellationToken;

const WRITE_CHUNK: usize = 1 << 20;
const MIN_STEAL: u64 = 4 << 20;
const MAX_RETRIES: u32 = 5;
const SAVE_EVERY: Duration = Duration::from_secs(5);
pub const MAX_TORRENT_FILE_BYTES: usize = 10 * 1024 * 1024;

pub struct HttpDownloadResult {
    pub download: Download,
    pub is_torrent_file: bool,
}

pub struct HttpDownloader {
    download: Download,
    max_connections: usize,
    min_split_size: u64,
    cancel_token: CancellationToken,
    pub downloaded: Arc<AtomicU64>,
    pub active_conns: Arc<AtomicU32>,
    pub total_size: Arc<AtomicU64>,
    pub save_path: Arc<Mutex<Option<String>>>,
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
            save_path: Arc::new(Mutex::new(None)),
        }
    }

    pub async fn run(&mut self) -> anyhow::Result<HttpDownloadResult> {
        crate::worker::check_target(&self.download.url)?;
        let client = crate::worker::http_client()?;
        let probe = tokio::select! {
            r = probe(&client, &self.download.url) => r?,
            _ = self.cancel_token.cancelled() => anyhow::bail!("Download cancelled"),
        };
        self.total_size
            .store(probe.total.unwrap_or(0), Ordering::Relaxed);

        let current = PathBuf::from(&self.download.save_path);
        let (path, file, resume) = match read_state(&sidecar_path(&current))
            .await
            .filter(|state| state.owner == self.download.id)
        {
            Some(state) => {
                let len = tokio::fs::metadata(&current).await.map(|m| m.len()).ok();
                let resume = (probe.ranged
                    && probe.total == Some(state.total)
                    && probe.validator == state.validator
                    && len == Some(state.total))
                .then_some(state.remaining);
                let file = tokio::fs::OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(resume.is_none())
                    .open(&current)
                    .await?;
                (current, file, resume)
            }
            None => {
                let wanted = match &probe.filename {
                    Some(name) => current.with_file_name(name),
                    None => current,
                };
                let (path, file) = create_unique(&wanted).await?;
                (path, file, None)
            }
        };

        self.download.save_path = path.to_string_lossy().into_owned();
        self.download.filename = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        *self.save_path.lock().unwrap() = Some(self.download.save_path.clone());

        let is_torrent_from_header = probe.content_type.contains("application/x-bittorrent");
        let known_total = probe.total;

        let transfer = Transfer {
            client,
            url: self.download.url.clone(),
            max_conns: self.max_connections,
            min_split: self.min_split_size,
            downloaded: Arc::clone(&self.downloaded),
            active_conns: Arc::clone(&self.active_conns),
            cancel: self.cancel_token.clone(),
        };
        transfer
            .run(
                file.into_std().await,
                probe,
                resume,
                Some((sidecar_path(&path), self.download.id.clone())),
            )
            .await?;

        let downloaded = self.downloaded.load(Ordering::Relaxed);
        self.download.status = DownloadStatus::Completed;
        self.download.speed = 0;
        self.download.downloaded_size = downloaded;
        self.download.total_size = known_total.unwrap_or(downloaded);

        let is_torrent = downloaded <= MAX_TORRENT_FILE_BYTES as u64
            && (is_torrent_from_header
                || self.download.filename.ends_with(".torrent")
                || check_torrent_magic(&self.download.save_path).await);

        Ok(HttpDownloadResult {
            download: self.download.clone(),
            is_torrent_file: is_torrent,
        })
    }
}

pub struct Probe {
    pub total: Option<u64>,
    pub ranged: bool,
    pub filename: Option<String>,
    pub content_type: String,
    validator: Option<String>,
    response: Option<reqwest::Response>,
}

pub async fn probe(client: &reqwest::Client, url: &str) -> anyhow::Result<Probe> {
    let mut resp = client.get(url).header(RANGE, "bytes=0-").send().await?;
    let ranged_total = (resp.status() == StatusCode::PARTIAL_CONTENT)
        .then(|| {
            resp.headers()
                .get(CONTENT_RANGE)
                .and_then(|v| v.to_str().ok())
                .and_then(parse_content_range)
        })
        .flatten()
        .filter(|(start, _)| *start == 0)
        .and_then(|(_, total)| total);
    if resp.status() == StatusCode::RANGE_NOT_SATISFIABLE
        || (resp.status() == StatusCode::PARTIAL_CONTENT && ranged_total.is_none())
    {
        resp = client.get(url).send().await?;
    }
    let status = resp.status();
    if !status.is_success() {
        anyhow::bail!("server returned HTTP {}", status);
    }

    let headers = resp.headers();
    let header = |name| headers.get(name).and_then(|v| v.to_str().ok());
    let filename = header(CONTENT_DISPOSITION)
        .and_then(extract_filename_from_disposition)
        .or_else(|| {
            resp.url()
                .path_segments()
                .and_then(|mut s| s.next_back())
                .filter(|s| s.contains('.'))
                .map(str::to_string)
        })
        .map(|n| sanitize_filename(&n));
    let content_type = header(CONTENT_TYPE).unwrap_or("").to_lowercase();
    let validator = header(ETAG).or(header(LAST_MODIFIED)).map(str::to_string);

    let (total, ranged) = match ranged_total {
        Some(total) if status == StatusCode::PARTIAL_CONTENT => (Some(total), true),
        _ => (resp.content_length(), false),
    };

    Ok(Probe {
        total,
        ranged,
        filename,
        content_type,
        validator,
        response: Some(resp),
    })
}

fn parse_content_range(value: &str) -> Option<(u64, Option<u64>)> {
    let (range, total) = value.trim().strip_prefix("bytes ")?.split_once('/')?;
    let start = range.split_once('-')?.0.trim().parse().ok()?;
    Some((start, total.trim().parse().ok()))
}

pub fn sidecar_path(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".dload");
    PathBuf::from(s)
}

async fn create_unique(path: &Path) -> anyhow::Result<(PathBuf, tokio::fs::File)> {
    let mut stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut ext = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    if let Some(base) = stem.strip_suffix(".tar") {
        ext = format!(".tar{}", ext);
        stem = base.to_string();
    }
    for n in 0..1000 {
        let candidate = if n == 0 {
            path.to_path_buf()
        } else {
            path.with_file_name(format!("{} ({}){}", stem, n, ext))
        };
        match tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
            .await
        {
            Ok(file) => return Ok((candidate, file)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    }
    anyhow::bail!("no free filename for {}", path.display())
}

#[derive(serde::Serialize, serde::Deserialize)]
struct ResumeState {
    owner: String,
    total: u64,
    validator: Option<String>,
    remaining: Vec<(u64, u64)>,
}

async fn read_state(sidecar: &Path) -> Option<ResumeState> {
    serde_json::from_slice(&tokio::fs::read(sidecar).await.ok()?).ok()
}

async fn write_state(sidecar: &Path, state: &ResumeState) -> std::io::Result<()> {
    let mut tmp = sidecar.as_os_str().to_owned();
    tmp.push(".tmp");
    tokio::fs::write(&tmp, serde_json::to_vec(state)?).await?;
    tokio::fs::rename(&tmp, sidecar).await
}

struct Seg {
    pos: u64,
    end: u64,
    done: u64,
    owned: bool,
}

impl Seg {
    fn new(start: u64, end: u64) -> Self {
        Self {
            pos: start,
            end,
            done: start,
            owned: false,
        }
    }
}

fn next_work(segs: &mut Vec<Seg>) -> Option<usize> {
    if let Some(i) = segs.iter().position(|s| !s.owned && s.pos < s.end) {
        segs[i].owned = true;
        return Some(i);
    }
    let (i, rem) = segs
        .iter()
        .enumerate()
        .filter(|(_, s)| s.owned)
        .map(|(i, s)| (i, s.end - s.pos))
        .max_by_key(|&(_, rem)| rem)?;
    if rem < MIN_STEAL {
        return None;
    }
    let mid = segs[i].pos + rem / 2;
    let end = std::mem::replace(&mut segs[i].end, mid);
    let mut seg = Seg::new(mid, end);
    seg.owned = true;
    segs.push(seg);
    Some(segs.len() - 1)
}

enum SegError {
    Retry(anyhow::Error),
    Fatal(anyhow::Error),
}

impl SegError {
    fn into_inner(self) -> anyhow::Error {
        match self {
            SegError::Retry(e) | SegError::Fatal(e) => e,
        }
    }
}

struct Ctx {
    client: reqwest::Client,
    url: String,
    file: std::fs::File,
    total: u64,
    segs: Mutex<Vec<Seg>>,
    downloaded: Arc<AtomicU64>,
    cancel: CancellationToken,
}

impl Ctx {
    fn range(&self, idx: usize) -> (u64, u64) {
        let segs = self.segs.lock().unwrap();
        (segs[idx].pos, segs[idx].end)
    }

    fn claim(&self, idx: usize, len: usize) -> (usize, bool) {
        let mut segs = self.segs.lock().unwrap();
        let seg = &mut segs[idx];
        let take = (seg.end - seg.pos).min(len as u64);
        seg.pos += take;
        (take as usize, seg.pos >= seg.end)
    }

    fn remaining(&self) -> Vec<(u64, u64)> {
        self.segs
            .lock()
            .unwrap()
            .iter()
            .filter(|s| s.done < s.end)
            .map(|s| (s.done, s.end))
            .collect()
    }
}

pub struct Transfer {
    pub client: reqwest::Client,
    pub url: String,
    pub max_conns: usize,
    pub min_split: u64,
    pub downloaded: Arc<AtomicU64>,
    pub active_conns: Arc<AtomicU32>,
    pub cancel: CancellationToken,
}

impl Transfer {
    pub async fn run(
        &self,
        file: std::fs::File,
        mut probe: Probe,
        resume: Option<Vec<(u64, u64)>>,
        sidecar: Option<(PathBuf, String)>,
    ) -> anyhow::Result<()> {
        let total = probe.total.unwrap_or(0);
        let save = |remaining: Vec<(u64, u64)>| {
            let sidecar = sidecar.clone();
            let validator = probe.validator.clone();
            async move {
                if let Some((sidecar, owner)) = sidecar {
                    let state = ResumeState {
                        owner,
                        total,
                        validator,
                        remaining,
                    };
                    if let Err(e) = write_state(&sidecar, &state).await {
                        tracing::warn!("Failed to save resume state {}: {}", sidecar.display(), e);
                    }
                }
            }
        };
        let resuming = resume.is_some();
        let segs = match resume {
            Some(remaining) => remaining
                .into_iter()
                .map(|(start, end)| Seg::new(start, end))
                .collect(),
            None if probe.ranged => {
                file.set_len(total)?;
                let n =
                    (total / self.min_split.max(1 << 20)).clamp(1, self.max_conns.max(1) as u64);
                (0..n)
                    .map(|i| Seg::new(total * i / n, total * (i + 1) / n))
                    .collect()
            }
            None => vec![Seg::new(0, probe.total.unwrap_or(u64::MAX))],
        };
        let left: u64 = segs.iter().map(|s| s.end - s.pos).sum();
        if probe.ranged {
            self.downloaded.store(total - left, Ordering::Relaxed);
        }
        if !resuming {
            save(vec![(0, total)]).await;
        }

        let ctx = Arc::new(Ctx {
            client: self.client.clone(),
            url: self.url.clone(),
            file,
            total,
            segs: Mutex::new(segs),
            downloaded: Arc::clone(&self.downloaded),
            cancel: self.cancel.child_token(),
        });

        if !probe.ranged {
            let resp = probe
                .response
                .take()
                .ok_or_else(|| anyhow::anyhow!("missing response body"))?;
            ctx.segs.lock().unwrap()[0].owned = true;
            self.active_conns.store(1, Ordering::Relaxed);
            let result = stream_into(&ctx, 0, resp).await;
            self.active_conns.store(0, Ordering::Relaxed);
            result.map_err(SegError::into_inner)?;
            let got = ctx.range(0).0;
            if probe.total.is_some_and(|t| got < t) {
                anyhow::bail!("connection closed after {} of {} bytes", got, total);
            }
            remove_sidecar(sidecar.as_ref()).await;
            return Ok(());
        }

        let n = (left / self.min_split.max(1 << 20)).clamp(1, self.max_conns.max(1) as u64);
        let mut set = JoinSet::new();
        let mut first = probe.response.take();
        for _ in 0..n {
            let Some(idx) = next_work(&mut ctx.segs.lock().unwrap()) else {
                break;
            };
            let resp = if ctx.range(idx).0 == 0 {
                first.take()
            } else {
                None
            };
            spawn_segment(&mut set, &ctx, idx, resp);
        }
        drop(first);
        self.active_conns.store(set.len() as u32, Ordering::Relaxed);

        let mut tick = tokio::time::interval(SAVE_EVERY);
        tick.tick().await;
        let mut failure: Option<anyhow::Error> = None;
        loop {
            let joined = tokio::select! {
                j = set.join_next() => j,
                _ = tick.tick() => {
                    save(ctx.remaining()).await;
                    continue;
                }
            };
            let Some(joined) = joined else { break };
            match joined {
                Ok((_, Ok(()))) => {
                    if !ctx.cancel.is_cancelled() {
                        if let Some(idx) = next_work(&mut ctx.segs.lock().unwrap()) {
                            spawn_segment(&mut set, &ctx, idx, None);
                        }
                    }
                }
                Ok((idx, Err(SegError::Retry(e)))) => {
                    tracing::warn!("Connection gave up on {}: {:#}", self.url, e);
                    ctx.segs.lock().unwrap()[idx].owned = false;
                    failure = Some(e);
                }
                Ok((_, Err(SegError::Fatal(e)))) => {
                    ctx.cancel.cancel();
                    failure.get_or_insert(e);
                }
                Err(e) => {
                    ctx.cancel.cancel();
                    failure.get_or_insert(anyhow::anyhow!("download task panicked: {}", e));
                }
            }
            self.active_conns.store(set.len() as u32, Ordering::Relaxed);
        }

        let remaining = ctx.remaining();
        if remaining.is_empty() {
            remove_sidecar(sidecar.as_ref()).await;
            return Ok(());
        }
        save(remaining).await;
        if self.cancel.is_cancelled() {
            anyhow::bail!("Download cancelled");
        }
        Err(failure.unwrap_or_else(|| anyhow::anyhow!("download incomplete")))
    }
}

async fn remove_sidecar(sidecar: Option<&(PathBuf, String)>) {
    if let Some((sidecar, _)) = sidecar {
        let _ = tokio::fs::remove_file(sidecar).await;
    }
}

fn spawn_segment(
    set: &mut JoinSet<(usize, Result<(), SegError>)>,
    ctx: &Arc<Ctx>,
    idx: usize,
    first: Option<reqwest::Response>,
) {
    let ctx = Arc::clone(ctx);
    set.spawn(async move { (idx, fetch_segment(&ctx, idx, first).await) });
}

async fn fetch_segment(
    ctx: &Arc<Ctx>,
    idx: usize,
    mut first: Option<reqwest::Response>,
) -> Result<(), SegError> {
    let mut failures = 0u32;
    loop {
        let (pos, end) = ctx.range(idx);
        if pos >= end {
            return Ok(());
        }
        let outcome = match first.take() {
            Some(resp) => stream_into(ctx, idx, resp).await,
            None => match request_range(ctx, pos, end).await {
                Ok(resp) => stream_into(ctx, idx, resp).await,
                Err(e) => Err(e),
            },
        };
        let err = match outcome {
            Err(SegError::Fatal(e)) => return Err(SegError::Fatal(e)),
            _ if ctx.range(idx).0 > pos => {
                failures = 0;
                continue;
            }
            Err(SegError::Retry(e)) => e,
            Ok(()) => anyhow::anyhow!("server closed the connection at byte {}", pos),
        };
        failures += 1;
        if failures >= MAX_RETRIES {
            return Err(SegError::Retry(err));
        }
        let backoff = Duration::from_secs(1 << (failures - 1));
        tracing::debug!(
            "Range {}-{} retry {} in {:?}: {:#}",
            pos,
            end,
            failures,
            backoff,
            err
        );
        tokio::select! {
            _ = tokio::time::sleep(backoff) => {}
            _ = ctx.cancel.cancelled() => return Err(SegError::Fatal(anyhow::anyhow!("Download cancelled"))),
        }
    }
}

async fn request_range(ctx: &Ctx, pos: u64, end: u64) -> Result<reqwest::Response, SegError> {
    let send = ctx
        .client
        .get(&ctx.url)
        .header(RANGE, format!("bytes={}-{}", pos, end - 1))
        .send();
    let resp = tokio::select! {
        r = send => r.map_err(|e| SegError::Retry(e.into()))?,
        _ = ctx.cancel.cancelled() => return Err(SegError::Fatal(anyhow::anyhow!("Download cancelled"))),
    };
    let status = resp.status();
    if status == StatusCode::OK && pos == 0 {
        return Ok(resp);
    }
    if status != StatusCode::PARTIAL_CONTENT {
        return Err(SegError::Retry(anyhow::anyhow!(
            "server returned HTTP {} for bytes {}-{}",
            status,
            pos,
            end - 1
        )));
    }
    let content_range = resp
        .headers()
        .get(CONTENT_RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(parse_content_range);
    if let Some((start, total)) = content_range {
        if start != pos || total.is_some_and(|t| t != ctx.total) {
            return Err(SegError::Fatal(anyhow::anyhow!(
                "server returned a different byte range than requested; the remote file may have changed"
            )));
        }
    }
    Ok(resp)
}

async fn stream_into(ctx: &Arc<Ctx>, idx: usize, resp: reqwest::Response) -> Result<(), SegError> {
    let mut stream = resp.bytes_stream();
    let mut buf = Vec::with_capacity(WRITE_CHUNK);
    let mut at = ctx.range(idx).0;
    let mut pending = None;
    let result = loop {
        let next = tokio::select! {
            biased;
            _ = ctx.cancel.cancelled() => break Err(SegError::Fatal(anyhow::anyhow!("Download cancelled"))),
            next = stream.next() => next,
        };
        let chunk = match next {
            None => break Ok(()),
            Some(Err(e)) => break Err(SegError::Retry(e.into())),
            Some(Ok(chunk)) => chunk,
        };
        let (take, finished) = ctx.claim(idx, chunk.len());
        buf.extend_from_slice(&chunk[..take]);
        ctx.downloaded.fetch_add(take as u64, Ordering::Relaxed);
        if buf.len() >= WRITE_CHUNK {
            if let Err(e) = write_behind(ctx, idx, &mut pending, &mut buf, &mut at).await {
                return Err(SegError::Fatal(e));
            }
        }
        if finished {
            break Ok(());
        }
    };
    let flushed = match write_behind(ctx, idx, &mut pending, &mut buf, &mut at).await {
        Ok(()) => settle(ctx, idx, &mut pending).await,
        Err(e) => Err(e),
    };
    match flushed {
        Ok(()) => result,
        Err(e) => Err(SegError::Fatal(e)),
    }
}

async fn write_behind(
    ctx: &Arc<Ctx>,
    idx: usize,
    pending: &mut Option<JoinHandle<std::io::Result<u64>>>,
    buf: &mut Vec<u8>,
    at: &mut u64,
) -> anyhow::Result<()> {
    settle(ctx, idx, pending).await?;
    if buf.is_empty() {
        return Ok(());
    }
    let data = std::mem::replace(buf, Vec::with_capacity(WRITE_CHUNK));
    let offset = *at;
    *at += data.len() as u64;
    let ctx = Arc::clone(ctx);
    *pending = Some(tokio::task::spawn_blocking(move || {
        ctx.file
            .write_all_at(&data, offset)
            .map(|()| offset + data.len() as u64)
    }));
    Ok(())
}

async fn settle(
    ctx: &Ctx,
    idx: usize,
    pending: &mut Option<JoinHandle<std::io::Result<u64>>>,
) -> anyhow::Result<()> {
    if let Some(handle) = pending.take() {
        let done = handle.await??;
        ctx.segs.lock().unwrap()[idx].done = done;
    }
    Ok(())
}

pub async fn fetch_small(url: &str, max_bytes: usize) -> anyhow::Result<Vec<u8>> {
    crate::worker::check_target(url)?;
    let resp = crate::worker::http_client()?
        .get(url)
        .send()
        .await?
        .error_for_status()?;
    if resp.content_length().is_some_and(|n| n > max_bytes as u64) {
        anyhow::bail!("response larger than {} bytes", max_bytes);
    }
    let mut body = Vec::new();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        body.extend_from_slice(&chunk?);
        if body.len() > max_bytes {
            anyhow::bail!("response larger than {} bytes", max_bytes);
        }
    }
    Ok(body)
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
    use tokio::io::AsyncReadExt;
    let Ok(mut file) = tokio::fs::File::open(path).await else {
        return false;
    };
    let mut head = [0u8; 11];
    if file.read_exact(&mut head).await.is_err() {
        return false;
    }
    head.starts_with(b"d8:announce")
        || head.starts_with(b"d13:")
        || head.starts_with(b"d7:comment")
        || head.starts_with(b"d4:info")
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── segmented transfer ──────────────────────────────────────────────

    fn sample_bytes(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i * 31 % 251) as u8).collect()
    }

    async fn serve(dir: &Path) -> String {
        let app = axum::Router::new()
            .route(
                "/norange.bin",
                axum::routing::get(|| async { sample_bytes(3 << 20) }),
            )
            .fallback_service(tower_http::services::ServeDir::new(dir));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{}", addr)
    }

    fn transfer_for(url: String, max_conns: usize) -> Transfer {
        Transfer {
            client: reqwest::Client::builder().http1_only().build().unwrap(),
            url,
            max_conns,
            min_split: 1 << 20,
            downloaded: Arc::new(AtomicU64::new(0)),
            active_conns: Arc::new(AtomicU32::new(0)),
            cancel: CancellationToken::new(),
        }
    }

    #[tokio::test]
    async fn multi_connection_download_is_byte_exact() {
        let src = tempfile::tempdir().unwrap();
        let data = sample_bytes(30 * 1024 * 1024 + 123);
        std::fs::write(src.path().join("big.bin"), &data).unwrap();
        let base = serve(src.path()).await;

        let out = tempfile::tempdir().unwrap();
        let path = out.path().join("big.bin");
        let sidecar = sidecar_path(&path);
        let t = transfer_for(format!("{}/big.bin", base), 8);
        let probe = probe(&t.client, &t.url).await.unwrap();
        assert!(probe.ranged);
        assert_eq!(probe.total, Some(data.len() as u64));
        let file = std::fs::File::create(&path).unwrap();
        t.run(file, probe, None, Some((sidecar.clone(), "id".to_string())))
            .await
            .unwrap();

        assert!(std::fs::read(&path).unwrap() == data);
        assert_eq!(t.downloaded.load(Ordering::Relaxed), data.len() as u64);
        assert!(!sidecar.exists());
    }

    #[tokio::test]
    async fn resume_fetches_only_remaining_ranges() {
        let src = tempfile::tempdir().unwrap();
        let data = sample_bytes(8 * 1024 * 1024);
        std::fs::write(src.path().join("r.bin"), &data).unwrap();
        let base = serve(src.path()).await;

        let out = tempfile::tempdir().unwrap();
        let path = out.path().join("r.bin");
        let mut partial = data.clone();
        partial[1_000_000..3_000_000].fill(0);
        partial[6_000_000..].fill(0);
        std::fs::write(&path, &partial).unwrap();

        let t = transfer_for(format!("{}/r.bin", base), 4);
        let probe = probe(&t.client, &t.url).await.unwrap();
        let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        let remaining = vec![(1_000_000, 3_000_000), (6_000_000, data.len() as u64)];
        t.run(file, probe, Some(remaining), None).await.unwrap();

        assert!(std::fs::read(&path).unwrap() == data);
    }

    #[tokio::test]
    async fn server_without_range_support_streams_once() {
        let src = tempfile::tempdir().unwrap();
        let base = serve(src.path()).await;
        let out = tempfile::tempdir().unwrap();
        let path = out.path().join("n.bin");

        let t = transfer_for(format!("{}/norange.bin", base), 8);
        let probe = probe(&t.client, &t.url).await.unwrap();
        assert!(!probe.ranged);
        let file = std::fs::File::create(&path).unwrap();
        t.run(file, probe, None, None).await.unwrap();

        assert!(std::fs::read(&path).unwrap() == sample_bytes(3 << 20));
    }

    #[tokio::test]
    async fn probe_reports_http_errors() {
        let src = tempfile::tempdir().unwrap();
        let base = serve(src.path()).await;
        let client = reqwest::Client::new();
        let err = probe(&client, &format!("{}/missing.bin", base))
            .await
            .err()
            .unwrap();
        assert!(err.to_string().contains("404"));
    }

    #[test]
    fn next_work_adopts_orphans_then_splits_largest() {
        let mut segs = vec![Seg::new(0, 100 << 20), Seg::new(100 << 20, 120 << 20)];
        segs[0].owned = true;
        assert_eq!(next_work(&mut segs), Some(1));
        assert_eq!(next_work(&mut segs), Some(2));
        assert_eq!(segs[0].end, 50 << 20);
        assert_eq!((segs[2].pos, segs[2].end), (50 << 20, 100 << 20));

        let mut tiny = vec![Seg::new(0, 1 << 20)];
        tiny[0].owned = true;
        assert_eq!(next_work(&mut tiny), None);
    }

    #[test]
    fn content_range_parsing() {
        assert_eq!(
            parse_content_range("bytes 0-99/1000"),
            Some((0, Some(1000)))
        );
        assert_eq!(parse_content_range("bytes 5-9/*"), Some((5, None)));
        assert_eq!(parse_content_range("items 0-1/2"), None);
    }

    #[tokio::test]
    async fn create_unique_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let wanted = dir.path().join("file.zip");
        std::fs::write(&wanted, b"keep").unwrap();
        let (path, _) = create_unique(&wanted).await.unwrap();
        assert_eq!(path, dir.path().join("file (1).zip"));
        assert_eq!(std::fs::read(&wanted).unwrap(), b"keep");

        let tarball = dir.path().join("src.tar.gz");
        std::fs::write(&tarball, b"keep").unwrap();
        let (path, _) = create_unique(&tarball).await.unwrap();
        assert_eq!(path, dir.path().join("src (1).tar.gz"));
    }

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
