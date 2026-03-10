# HTTP Mirror for Torrent Downloads — Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Allow users to add an HTTP download link (from a seedbox) to an active torrent download, so files are fetched via HTTP and the torrent re-verifies them — achieving full speed while preserving arr stack integration.

**Architecture:** Pause the torrent in librqbit, download file(s) via HTTP to the same disk location, re-add the torrent so librqbit's `initial_check()` verifies existing data. The DLoad `Download` record (id, info_hash, category) is never deleted — only the ephemeral librqbit session handle is recycled. For zip files (multi-file torrents from seedbox), download to temp dir, extract, then re-add.

**Tech Stack:** Rust, librqbit v8, axum, reqwest, rusqlite, zip crate, vanilla JS frontend

**Spec:** `docs/superpowers/specs/2026-03-10-http-mirror-for-torrents-design.md`

---

## Chunk 1: Domain & DB Layer

### Task 1: Add mirror fields to Download struct

**Files:**
- Modify: `src/domain/download.rs:53-107`

- [ ] **Step 1: Add fields to Download struct**

In `src/domain/download.rs`, add two new fields to the `Download` struct after `content_path`:

```rust
pub http_mirror_status: Option<String>,
pub http_mirror_url: Option<String>,
```

- [ ] **Step 2: Update `Download::new()` to initialize the new fields**

In the `Self { ... }` block of `Download::new()`, add:

```rust
http_mirror_status: None,
http_mirror_url: None,
```

- [ ] **Step 3: Run `cargo check`**

Run: `cargo check`
Expected: PASS (fields added but not yet used in DB)

- [ ] **Step 4: Commit**

```bash
git add src/domain/download.rs
git commit -m "feat: add http_mirror_status and http_mirror_url fields to Download"
```

### Task 2: Add DB migration and update repository queries

**Files:**
- Modify: `src/db/mod.rs:57-61`
- Modify: `src/db/repository.rs:14-117`

- [ ] **Step 1: Add ALTER TABLE migration**

In `src/db/mod.rs`, after the existing `ALTER TABLE` block (line 57-61), add a new migration block:

```rust
let _ = conn.execute_batch(
    "ALTER TABLE downloads ADD COLUMN http_mirror_status TEXT;
     ALTER TABLE downloads ADD COLUMN http_mirror_url TEXT;",
);
```

Use `let _ =` because ALTER TABLE fails silently if columns already exist (same pattern as existing migrations).

- [ ] **Step 2: Update `insert_download` query**

In `src/db/repository.rs`, update `insert_download` to include the two new columns. Add `http_mirror_status, http_mirror_url` to the column list and `?18, ?19` to VALUES, then add to the params:

```rust
download.http_mirror_status,
download.http_mirror_url,
```

- [ ] **Step 3: Update `update_download` query**

Add `http_mirror_status=?15, http_mirror_url=?16` to the SET clause. Update the WHERE clause param index accordingly. Add to params:

```rust
download.http_mirror_status,
download.http_mirror_url,
```

- [ ] **Step 4: Update `get_all_downloads` query**

Add `http_mirror_status, http_mirror_url` to the SELECT column list. In the `query_map` closure, add after `content_path: row.get(16)?`:

```rust
http_mirror_status: row.get(17)?,
http_mirror_url: row.get(18)?,
```

- [ ] **Step 5: Run `cargo check`**

Run: `cargo check`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add src/db/mod.rs src/db/repository.rs
git commit -m "feat: add http_mirror columns to downloads table"
```

### Task 3: Add startup cleanup for stale mirrors

**Files:**
- Modify: `src/manager/mod.rs:26-65`

- [ ] **Step 1: Add mirror cleanup in `ManagerState::new()`**

In the `for mut dl in dl_list` loop inside `ManagerState::new()`, after the existing block that sets Seeding → Completed (lines 41-45), add:

```rust
// Clean up interrupted mirror operations from previous run
if dl.http_mirror_status.is_some() {
    dl.http_mirror_status = None;
    dl.http_mirror_url = None;
    let _ = repo.update_download(&dl);

    // Clean up any leftover temp zip file
    let data_dir = std::env::var("DLOAD_DATA_DIR").unwrap_or_else(|_| "/data".to_string());
    let tmp_zip = std::path::Path::new(&data_dir)
        .join(".tmp")
        .join(format!("{}.zip", dl.id));
    if tmp_zip.exists() {
        let _ = std::fs::remove_file(&tmp_zip);
    }
}
```

- [ ] **Step 2: Run `cargo check`**

Run: `cargo check`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add src/manager/mod.rs
git commit -m "feat: clean up stale http mirror state on startup"
```

---

## Chunk 2: Mirror Download Engine

### Task 4: Add zip dependency

**Files:**
- Modify: `Cargo.toml`

- [ ] **Step 1: Add zip crate**

In `Cargo.toml`, add after the `bytes = "1"` line:

```toml
zip = "2"
```

- [ ] **Step 2: Run `cargo check`**

Run: `cargo check`
Expected: PASS (zip crate downloads and compiles)

- [ ] **Step 3: Commit**

```bash
git add Cargo.toml
git commit -m "feat: add zip crate for multi-file torrent mirror extraction"
```

### Task 5: Implement dedicated mirror HTTP download function

**Files:**
- Create: `src/worker/mirror.rs`
- Modify: `src/worker/mod.rs`

- [ ] **Step 1: Create `src/worker/mirror.rs`**

This is a simplified HTTP downloader that writes to a fixed path without mutating any Download fields. It supports multi-connection range downloads (reusing the same pattern as `http.rs`) and reports progress via atomics.

```rust
use futures::stream::StreamExt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncSeekExt, AsyncWriteExt, BufWriter};
use tokio_util::sync::CancellationToken;

/// Downloads a file via HTTP to a fixed path without mutating any Download fields.
/// Returns (is_zip, total_bytes_downloaded).
pub struct MirrorDownloader {
    url: String,
    target_path: String,
    cancel_token: CancellationToken,
    pub downloaded: Arc<AtomicU64>,
    pub total_size: Arc<AtomicU64>,
}

pub struct MirrorResult {
    pub is_zip: bool,
    pub bytes_downloaded: u64,
}

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
```

- [ ] **Step 2: Register the module in `src/worker/mod.rs`**

Read `src/worker/mod.rs` and add `pub mod mirror;` to it.

- [ ] **Step 3: Run `cargo check`**

Run: `cargo check`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add src/worker/mirror.rs src/worker/mod.rs
git commit -m "feat: add dedicated mirror downloader with zip extraction"
```

---

## Chunk 3: Manager Mirror Orchestration

### Task 6: Implement `mirror_readd_torrent()` helper

**Files:**
- Modify: `src/manager/mod.rs`

This is the critical function that re-adds a torrent to librqbit without clobbering arr stack fields. It replicates the URL-type dispatch from `handle_torrent_download` but skips overwriting `filename`, `save_path`, `content_path`.

- [ ] **Step 1: Add `mirror_readd_torrent` method**

Add this method to the `impl ManagerState` block (the one starting at line 843 with `monitor_torrent`):

```rust
/// Re-add a torrent to librqbit after HTTP mirror download.
/// Unlike `session_add_and_wait`, this does NOT overwrite filename/save_path/content_path
/// to preserve arr stack data.
async fn mirror_readd_torrent(
    self: &Arc<Self>,
    download: &mut Download,
    output_dir: &str,
) -> anyhow::Result<usize> {
    let url = download.url.clone();
    let add = if url.starts_with("magnet:") {
        librqbit::AddTorrent::from_url(&url)
    } else if url.starts_with("torrent://") {
        let download_dir = self.download_dir().await;
        let torrent_file = std::path::Path::new(&download_dir)
            .join(".torrents")
            .join(format!("{}.torrent", download.id));
        let bytes = tokio::fs::read(&torrent_file).await.map_err(|e| {
            anyhow::anyhow!("Failed to read persisted torrent file: {}", e)
        })?;
        librqbit::AddTorrent::from_bytes(bytes)
    } else {
        let resp = reqwest::get(&url).await?;
        let bytes = resp.bytes().await?;
        librqbit::AddTorrent::from_bytes(bytes)
    };

    let session = self.get_torrent_session().await?;
    let opts = librqbit::AddTorrentOptions {
        overwrite: true,
        output_folder: Some(output_dir.to_string()),
        force_tracker_interval: Some(std::time::Duration::from_secs(120)),
        ..Default::default()
    };

    let handle = session
        .add_torrent(add, Some(opts))
        .await
        .map_err(|e| anyhow::anyhow!("Failed to re-add torrent: {}", e))?
        .into_handle()
        .ok_or_else(|| anyhow::anyhow!("Torrent was a duplicate or couldn't get handle"))?;

    let torrent_id = handle.id();

    // Store handle mapping — but do NOT overwrite filename/save_path/content_path
    {
        let mut handles = self.torrent_handles.write().await;
        handles.insert(download.id.clone(), torrent_id);
    }

    // Only update info_hash if not already set
    let hash_str = handle.info_hash().as_string();
    if download.info_hash.is_none() {
        download.info_hash = Some(hash_str);
    }

    Ok(torrent_id)
}
```

- [ ] **Step 2: Run `cargo check`**

Run: `cargo check`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add src/manager/mod.rs
git commit -m "feat: add mirror_readd_torrent helper that preserves arr stack fields"
```

### Task 7: Implement `start_http_mirror()` orchestrator

**Files:**
- Modify: `src/manager/mod.rs`

- [ ] **Step 1: Add `start_http_mirror` method**

Add this public method to the main `impl ManagerState` block (the first one, after `get_download` at line 745):

```rust
/// Start an HTTP mirror for a torrent download.
/// Downloads file(s) via HTTP, then re-adds the torrent for hash verification.
pub async fn start_http_mirror(
    self: Arc<Self>,
    id: String,
    mirror_url: String,
    keep_seeding: bool,
) {
    let state = Arc::clone(&self);

    tokio::spawn(async move {
        if let Err(e) = state
            .run_http_mirror(id.clone(), mirror_url, keep_seeding)
            .await
        {
            tracing::error!("HTTP mirror failed for {}: {}", id, e);
            // Clear mirror status and set error
            let mut downloads = state.downloads.write().await;
            if let Some(d) = downloads.get_mut(&id) {
                d.http_mirror_status = None;
                d.http_mirror_url = None;
                d.error_message = Some(format!("HTTP mirror failed: {}", e));
            }
            if let Some(d) = downloads.get(&id).cloned() {
                drop(downloads);
                state.update_download(&d).await;
            }
        }
    });
}

async fn run_http_mirror(
    self: &Arc<Self>,
    id: String,
    mirror_url: String,
    keep_seeding: bool,
) -> anyhow::Result<()> {
    use crate::worker::mirror::{extract_zip_safe, MirrorDownloader};

    // 1. Get and validate download
    let download = self
        .get_download(&id)
        .await
        .ok_or_else(|| anyhow::anyhow!("Download not found"))?;

    if download.protocol != Protocol::Torrent {
        anyhow::bail!("Not a torrent download");
    }
    if download.http_mirror_status.is_some() {
        anyhow::bail!("Mirror already in progress");
    }

    // 2. Snapshot arr-safe fields
    let snapshot_save_path = download.save_path.clone();
    let snapshot_content_path = download.content_path.clone();
    let snapshot_filename = download.filename.clone();

    // Determine output_dir for re-add (parent of save_path, not global download_dir)
    let output_dir = match std::path::Path::new(&snapshot_save_path).parent() {
        Some(p) => p.to_string_lossy().to_string(),
        None => self.download_dir().await,
    };

    // 3. Set mirror status
    {
        let mut downloads = self.downloads.write().await;
        if let Some(d) = downloads.get_mut(&id) {
            d.http_mirror_status = Some("downloading".to_string());
            d.http_mirror_url = Some(mirror_url.clone());
        }
    }

    // 4. Cancel any existing monitoring token FIRST (so monitor_torrent exits cleanly)
    self.cancel_download(&id).await;
    // Brief yield to let monitor_torrent react to cancellation
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // 5. Pause torrent in librqbit and delete from session (keep files)
    let torrent_id = {
        let mut handles = self.torrent_handles.write().await;
        handles.remove(&id)
    };
    if let Some(tid) = torrent_id {
        if let Ok(session) = self.get_torrent_session().await {
            if let Some(handle) = session.get(TorrentIdOrHash::Id(tid)) {
                let _ = session.pause(&handle).await;
            }
            let _ = session.delete(TorrentIdOrHash::Id(tid), false).await;
        }
    }

    // 6. Register cancel token for HTTP download phase
    let http_cancel = self.register_cancel_token(&id).await;
    // Track whether HTTP phase was cancelled (for post-recheck pause)
    let was_cancelled = http_cancel.clone();

    // 7. Always download to a temp file first.
    // After download completes, MirrorResult.is_zip tells us whether to extract or move.
    // This handles the case where seedbox HEAD returns application/octet-stream
    // but GET returns application/zip.
    let tmp_dir = format!("{}/.tmp", output_dir);
    let tmp_download_path = format!("{}/{}.mirror", tmp_dir, id);
    let tmp_zip_path = format!("{}/{}.zip", tmp_dir, id);
    tokio::fs::create_dir_all(&tmp_dir).await?;

    let target_path = snapshot_content_path
        .as_deref()
        .unwrap_or(&snapshot_save_path);

    let downloader = MirrorDownloader::new(
        mirror_url.clone(),
        tmp_download_path.clone(),
        http_cancel.clone(),
    );
    let downloaded_atomic = Arc::clone(&downloader.downloaded);
    let total_size_atomic = Arc::clone(&downloader.total_size);

    let download_task = tokio::spawn(async move { downloader.run().await });

    // 8. Monitor loop
    let mut last_downloaded: u64 = 0;
    let mut last_time = std::time::Instant::now();
    let mut db_tick: u32 = 0;

    loop {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        if download_task.is_finished() {
            break;
        }

        if http_cancel.is_cancelled() {
            download_task.abort();
            break;
        }

        let current_downloaded = downloaded_atomic.load(std::sync::atomic::Ordering::Relaxed);
        let current_total = total_size_atomic.load(std::sync::atomic::Ordering::Relaxed);
        let now = std::time::Instant::now();
        let elapsed = now.duration_since(last_time).as_secs_f64();

        let speed = if elapsed > 0.0 {
            (current_downloaded.saturating_sub(last_downloaded) as f64 / elapsed) as u64
        } else {
            0
        };

        last_downloaded = current_downloaded;
        last_time = now;

        {
            let mut downloads = self.downloads.write().await;
            if let Some(d) = downloads.get_mut(&id) {
                d.downloaded_size = current_downloaded;
                d.speed = speed;
                d.upload_speed = 0;
                if current_total > 0 {
                    d.total_size = current_total;
                    d.progress = (current_downloaded as f64 / current_total as f64) * 100.0;
                    if speed > 0 {
                        let remaining = current_total.saturating_sub(current_downloaded);
                        let eta_secs = remaining / speed;
                        let hours = eta_secs / 3600;
                        let mins = (eta_secs % 3600) / 60;
                        let secs = eta_secs % 60;
                        d.eta = if hours > 0 {
                            Some(format!("{}h{}m{}s", hours, mins, secs))
                        } else if mins > 0 {
                            Some(format!("{}m{}s", mins, secs))
                        } else {
                            Some(format!("{}s", secs))
                        };
                    } else {
                        d.eta = None;
                    }
                }
            }
        }

        db_tick += 1;
        if db_tick >= 5 {
            db_tick = 0;
            if let Some(snap) = self.get_download(&id).await {
                let repo = Arc::clone(&self.repo);
                tokio::task::spawn_blocking(move || {
                    let _ = repo.update_download(&snap);
                });
            }
        }
    }

    // 9. Handle download result — use MirrorResult.is_zip for zip detection
    //    (from GET response Content-Type, not HEAD — per spec requirement)
    let (http_succeeded, is_zip) = match download_task.await {
        Ok(Ok(result)) => (true, result.is_zip),
        Ok(Err(e)) => {
            tracing::warn!("HTTP mirror download error (will still re-add torrent): {}", e);
            (false, false)
        }
        Err(_) => {
            tracing::warn!("HTTP mirror task aborted (will still re-add torrent)");
            (false, false)
        }
    };

    // 10. Move or extract the downloaded file
    if http_succeeded {
        if is_zip {
            // Zip flow: extract to output_dir
            {
                let mut downloads = self.downloads.write().await;
                if let Some(d) = downloads.get_mut(&id) {
                    d.http_mirror_status = Some("extracting".to_string());
                }
            }

            // Rename temp file to .zip extension for clarity
            let _ = tokio::fs::rename(&tmp_download_path, &tmp_zip_path).await;

            let extract_target = output_dir.clone();
            let zip_path = tmp_zip_path.clone();
            match tokio::task::spawn_blocking(move || {
                extract_zip_safe(&zip_path, &extract_target)
            })
            .await
            {
                Ok(Ok(files)) => {
                    tracing::info!("Extracted {} files from mirror zip", files.len());
                }
                Ok(Err(e)) => {
                    tracing::warn!("Zip extraction failed: {}", e);
                }
                Err(e) => {
                    tracing::warn!("Zip extraction task panicked: {}", e);
                }
            }

            // Clean up zip
            let _ = tokio::fs::remove_file(&tmp_zip_path).await;
        } else {
            // Single-file flow: move temp file to the torrent's target path
            if let Err(e) = tokio::fs::rename(&tmp_download_path, target_path).await {
                // rename can fail across filesystems; fall back to copy+delete
                if let Err(e2) = tokio::fs::copy(&tmp_download_path, target_path).await {
                    tracing::warn!("Failed to move mirror file: rename={}, copy={}", e, e2);
                }
                let _ = tokio::fs::remove_file(&tmp_download_path).await;
            }
        }
    }
    // Clean up temp file if it still exists (e.g., failed download)
    let _ = tokio::fs::remove_file(&tmp_download_path).await;

    // 11. Set rechecking status
    {
        let mut downloads = self.downloads.write().await;
        if let Some(d) = downloads.get_mut(&id) {
            d.http_mirror_status = Some("rechecking".to_string());
            d.speed = 0;
        }
    }

    // 12. Re-add torrent
    let mut download = self
        .get_download(&id)
        .await
        .ok_or_else(|| anyhow::anyhow!("Download disappeared during mirror"))?;

    let torrent_id = self
        .mirror_readd_torrent(&mut download, &output_dir)
        .await?;

    // 13. Restore snapshotted fields
    download.save_path = snapshot_save_path;
    download.content_path = snapshot_content_path;
    download.filename = snapshot_filename;
    download.status = DownloadStatus::Downloading;
    download.http_mirror_status = None;
    download.http_mirror_url = None;
    self.update_download(&download).await;

    // 14. If HTTP phase was cancelled by user, pause the re-added torrent immediately
    //     (spec: "torrent is then paused in librqbit")
    if was_cancelled.is_cancelled() {
        if let Ok(session) = self.get_torrent_session().await {
            if let Some(handle) = session.get(TorrentIdOrHash::Id(torrent_id)) {
                let _ = session.pause(&handle).await;
            }
        }
        let mut d = download;
        d.status = DownloadStatus::Paused;
        d.speed = 0;
        self.update_download(&d).await;
        return Ok(());
    }

    // 15. Register new cancel token and start monitoring
    let monitor_cancel = self.register_cancel_token(&id).await;
    let state = Arc::clone(self);
    state
        .monitor_torrent(id.clone(), torrent_id, monitor_cancel)
        .await;

    // 16. After monitoring: handle keep_seeding preference
    if !keep_seeding {
        let latest = self.get_download(&id).await;
        if let Some(d) = latest {
            if d.status == DownloadStatus::Seeding {
                self.cancel_download(&id).await;
                let mut stopped = d;
                stopped.status = DownloadStatus::Completed;
                stopped.upload_speed = 0;
                stopped.speed = 0;
                self.update_download(&stopped).await;
            }
        }
    }

    Ok(())
}

- [ ] **Step 2: Run `cargo check`**

Run: `cargo check`
Expected: PASS

- [ ] **Step 3: Run `cargo clippy --all-targets --all-features -- -D warnings`**

Expected: PASS with no warnings

- [ ] **Step 4: Commit**

```bash
git add src/manager/mod.rs
git commit -m "feat: implement HTTP mirror orchestrator with progress tracking"
```

---

## Chunk 4: API Endpoint

### Task 8: Add HTTP mirror API route

**Files:**
- Modify: `src/api/downloads.rs`

- [ ] **Step 1: Add request struct and handler**

In `src/api/downloads.rs`, add the request struct after the existing `DeleteParams`:

```rust
#[derive(serde::Deserialize)]
pub struct HttpMirrorRequest {
    url: String,
    #[serde(default = "default_true")]
    keep_seeding: bool,
}

fn default_true() -> bool {
    true
}
```

Then add the handler function:

```rust
async fn start_http_mirror(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    headers: axum::http::HeaderMap,
    Json(payload): Json<HttpMirrorRequest>,
) -> axum::response::Response {
    if let Err(e) = require_admin(extract_token(&headers)) {
        return axum::response::IntoResponse::into_response((
            axum::http::StatusCode::UNAUTHORIZED,
            e,
        ));
    }

    // Validate URL
    let url = payload.url.trim().to_string();
    if url.is_empty() {
        return axum::response::IntoResponse::into_response((
            axum::http::StatusCode::BAD_REQUEST,
            "URL is required",
        ));
    }

    // Check URL is HTTP/HTTPS
    match url::Url::parse(&url) {
        Ok(parsed) => {
            if parsed.scheme() != "http" && parsed.scheme() != "https" {
                return axum::response::IntoResponse::into_response((
                    axum::http::StatusCode::BAD_REQUEST,
                    "Only HTTP/HTTPS URLs are supported for mirrors",
                ));
            }
        }
        Err(_) => {
            return axum::response::IntoResponse::into_response((
                axum::http::StatusCode::BAD_REQUEST,
                "Invalid URL format",
            ));
        }
    }

    // Validate download exists and is eligible
    let download = state.get_download(&id).await;
    let download = match download {
        Some(d) => d,
        None => {
            return axum::response::IntoResponse::into_response((
                axum::http::StatusCode::NOT_FOUND,
                "Download not found",
            ));
        }
    };

    if download.protocol != crate::domain::Protocol::Torrent {
        return axum::response::IntoResponse::into_response((
            axum::http::StatusCode::BAD_REQUEST,
            "HTTP mirror is only available for torrent downloads",
        ));
    }

    if download.http_mirror_status.is_some() {
        return axum::response::IntoResponse::into_response((
            axum::http::StatusCode::CONFLICT,
            "HTTP mirror is already in progress for this download",
        ));
    }

    match download.status {
        DownloadStatus::Downloading
        | DownloadStatus::Paused
        | DownloadStatus::Seeding => {}
        _ => {
            return axum::response::IntoResponse::into_response((
                axum::http::StatusCode::BAD_REQUEST,
                "Download must be in Downloading, Paused, or Seeding status",
            ));
        }
    }

    // Start mirror asynchronously
    let state_clone = Arc::clone(&state);
    state_clone
        .start_http_mirror(id, url, payload.keep_seeding)
        .await;

    axum::response::IntoResponse::into_response((
        axum::http::StatusCode::OK,
        Json(serde_json::json!({ "success": true })),
    ))
}
```

- [ ] **Step 2: Register the route**

In the `router()` function, add before `.with_state(state)`:

```rust
.route("/api/downloads/:id/http-mirror", post(start_http_mirror))
```

- [ ] **Step 3: Run `cargo check`**

Run: `cargo check`
Expected: PASS

- [ ] **Step 4: Run `cargo clippy --all-targets --all-features -- -D warnings`**

Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/api/downloads.rs
git commit -m "feat: add POST /api/downloads/:id/http-mirror endpoint"
```

---

## Chunk 5: UI Changes

### Task 9: Add HTTP mirror UI in ellipsis menu

**Files:**
- Modify: `src/ui/app.js`

- [ ] **Step 1: Add "Add HTTP Mirror" button to ellipsis menu**

In `src/ui/app.js`, find the ellipsis menu section (around line 512-519). After the "Download .torrent" button (line 516-518) and before the closing `</div>` of the more-menu, add a conditional mirror button.

Replace the existing more-menu content block (lines 512-519):

```javascript
+ '<div class="more-menu" id="more-menu-' + safeId + '">'
+ '<button onclick="copyMagnet(event, \'' + safeId + '\')">'
+ 'Copy Magnet'
+ '</button>'
+ '<button onclick="downloadTorrent(event, \'' + safeId + '\')">'
+ 'Download .torrent'
+ '</button>'
+ (canMirror ? '<button onclick="showMirrorForm(event, \'' + safeId + '\')">'
+ 'Add HTTP Mirror'
+ '</button>' : '')
+ '</div>'
```

Then add the `canMirror` variable before the actions section. Find where `isTorrent` is defined and add after it:

```javascript
var canMirror = isTorrent && !d.http_mirror_status
    && (safeStatus === 'Downloading' || safeStatus === 'Paused' || safeStatus === 'Seeding');
```

- [ ] **Step 2: Add mirror form HTML**

After the more-dropdown div closing tag (after the `</div>` that closes the more-dropdown), add the inline mirror form:

```javascript
+ (isTorrent ? '<div class="mirror-form" id="mirror-form-' + safeId + '" style="display:none">'
+ '<input type="text" id="mirror-url-' + safeId + '" placeholder="HTTP/HTTPS mirror URL" class="mirror-input">'
+ '<label class="mirror-checkbox"><input type="checkbox" id="mirror-seed-' + safeId + '" checked> Keep seeding</label>'
+ '<div class="mirror-actions">'
+ '<button class="mirror-start-btn" onclick="startMirror(event, \'' + safeId + '\')">Start</button>'
+ '<button class="mirror-cancel-btn" onclick="hideMirrorForm(\'' + safeId + '\')">Cancel</button>'
+ '</div>'
+ '</div>' : '')
```

- [ ] **Step 3: Add mirror status label**

In the download info display area (around where speed/ETA are shown), add a mirror status indicator. Find the `connDisplay` area and add after ETA display:

```javascript
var mirrorLabel = d.http_mirror_status
    ? '<span class="mirror-status">' + ({
        'downloading': 'HTTP Mirror',
        'extracting': 'Extracting...',
        'rechecking': 'Rechecking...'
    }[d.http_mirror_status] || d.http_mirror_status) + '</span>'
    : '';
```

Include `mirrorLabel` in the HTML output after the ETA span.

- [ ] **Step 4: Add JavaScript handler functions**

At the end of the file (before the closing), add:

```javascript
function showMirrorForm(event, id) {
    event.stopPropagation();
    closeAllMenus();
    var form = document.getElementById('mirror-form-' + CSS.escape(id));
    if (form) {
        form.style.display = form.style.display === 'none' ? 'block' : 'none';
    }
}

function hideMirrorForm(id) {
    var form = document.getElementById('mirror-form-' + CSS.escape(id));
    if (form) form.style.display = 'none';
}

function startMirror(event, id) {
    event.stopPropagation();
    var urlInput = document.getElementById('mirror-url-' + CSS.escape(id));
    var seedCheckbox = document.getElementById('mirror-seed-' + CSS.escape(id));
    var url = urlInput ? urlInput.value.trim() : '';
    var keepSeeding = seedCheckbox ? seedCheckbox.checked : true;

    if (!url) {
        alert('Please enter a mirror URL');
        return;
    }

    var token = localStorage.getItem('auth_token');
    fetch('/api/downloads/' + encodeURIComponent(id) + '/http-mirror', {
        method: 'POST',
        headers: {
            'Content-Type': 'application/json',
            'Authorization': 'Bearer ' + token
        },
        body: JSON.stringify({ url: url, keep_seeding: keepSeeding })
    })
    .then(function(resp) {
        if (!resp.ok) {
            return resp.text().then(function(t) { throw new Error(t); });
        }
        return resp.json();
    })
    .then(function() {
        hideMirrorForm(id);
        refreshDownloads();
    })
    .catch(function(err) {
        alert('Failed to start mirror: ' + err.message);
    });
}

function closeAllMenus() {
    document.querySelectorAll('.more-menu').forEach(function(m) { m.classList.remove('open'); });
    document.querySelectorAll('.delete-menu').forEach(function(m) { m.classList.remove('open'); });
    openMenuId = null;
    openMoreMenuId = null;
}
```

- [ ] **Step 5: Add CSS for mirror form**

In `src/ui/style.css`, add styles for the mirror form:

```css
.mirror-form {
    padding: 8px 12px;
    background: var(--bg-secondary, #1e1e2e);
    border: 1px solid var(--border, #333);
    border-radius: 6px;
    margin-top: 6px;
}
.mirror-input {
    width: 100%;
    padding: 6px 8px;
    background: var(--bg-primary, #11111b);
    color: var(--text, #cdd6f4);
    border: 1px solid var(--border, #333);
    border-radius: 4px;
    font-size: 13px;
    margin-bottom: 6px;
    box-sizing: border-box;
}
.mirror-checkbox {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    color: var(--text-secondary, #a6adc8);
    margin-bottom: 6px;
    cursor: pointer;
}
.mirror-actions {
    display: flex;
    gap: 6px;
}
.mirror-start-btn {
    padding: 4px 12px;
    background: var(--accent, #89b4fa);
    color: var(--bg-primary, #11111b);
    border: none;
    border-radius: 4px;
    cursor: pointer;
    font-size: 12px;
    font-weight: 600;
}
.mirror-cancel-btn {
    padding: 4px 12px;
    background: transparent;
    color: var(--text-secondary, #a6adc8);
    border: 1px solid var(--border, #333);
    border-radius: 4px;
    cursor: pointer;
    font-size: 12px;
}
.mirror-status {
    font-size: 11px;
    color: var(--accent, #89b4fa);
    font-weight: 500;
    margin-left: 8px;
}
```

- [ ] **Step 6: Run `cargo build`**

Run: `cargo build`
Expected: PASS (UI files are embedded at compile time via `include_str!`)

- [ ] **Step 7: Commit**

```bash
git add src/ui/app.js src/ui/style.css
git commit -m "feat: add HTTP mirror UI in torrent ellipsis menu"
```

---

## Chunk 6: Build Verification & Cleanup

### Task 10: Full build and lint verification

**Files:** None (verification only)

- [ ] **Step 1: Run full build**

Run: `cargo build`
Expected: PASS

- [ ] **Step 2: Run clippy**

Run: `cargo clippy --all-targets --all-features -- -D warnings`
Expected: PASS with no warnings

- [ ] **Step 3: Run tests**

Run: `cargo test`
Expected: All existing tests pass

- [ ] **Step 4: Fix any issues found**

Address any compiler errors, clippy warnings, or test failures.

- [ ] **Step 5: Final commit if fixes were needed**

```bash
git add -A
git commit -m "fix: address build/lint issues in HTTP mirror implementation"
```
