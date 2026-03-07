use crate::db::repository::Repository;
use crate::domain::{Download, DownloadStatus, Protocol, Settings};
use crate::worker::http::HttpDownloader;
use librqbit::api::TorrentIdOrHash;
use librqbit::Session;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

/// Torrent session + the download_dir it was created with.
/// Reset when download_dir changes so torrents use the new path.
type TorrentSession = Arc<RwLock<Option<(String, Arc<Session>)>>>;

#[derive(Clone)]
pub struct ManagerState {
    pub downloads: Arc<RwLock<HashMap<String, Download>>>,
    pub settings: Arc<RwLock<Settings>>,
    pub repo: Arc<Repository>,
    torrent_session: TorrentSession,
    cancel_tokens: Arc<RwLock<HashMap<String, CancellationToken>>>,
    /// Maps download ID -> librqbit torrent handle ID for pause/resume
    torrent_handles: Arc<RwLock<HashMap<String, usize>>>,
}

impl ManagerState {
    pub fn new(settings: Settings, repo: Arc<Repository>) -> Self {
        // Load existing downloads from DB
        let downloads = match repo.get_all_downloads() {
            Ok(dl_list) => {
                let mut map = HashMap::new();
                for mut dl in dl_list {
                    // Downloads that were in-progress when the app stopped are now paused
                    if dl.status == DownloadStatus::Downloading {
                        dl.status = DownloadStatus::Paused;
                        dl.speed = 0;
                        dl.upload_speed = 0;
                        let _ = repo.update_download(&dl);
                    }
                    // Torrents that were seeding when the app stopped are now completed
                    if dl.status == DownloadStatus::Seeding {
                        dl.status = DownloadStatus::Completed;
                        dl.upload_speed = 0;
                        let _ = repo.update_download(&dl);
                    }
                    map.insert(dl.id.clone(), dl);
                }
                tracing::info!("Restored {} downloads from database", map.len());
                map
            }
            Err(e) => {
                tracing::error!("Failed to load downloads from database: {}", e);
                HashMap::new()
            }
        };

        Self {
            downloads: Arc::new(RwLock::new(downloads)),
            settings: Arc::new(RwLock::new(settings)),
            repo,
            torrent_session: Arc::new(RwLock::new(None)),
            cancel_tokens: Arc::new(RwLock::new(HashMap::new())),
            torrent_handles: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Read the current download directory from settings (always up-to-date).
    pub async fn download_dir(&self) -> String {
        self.settings.read().await.download_dir.clone()
    }

    /// Run a blocking repo operation off the async runtime.
    async fn repo_blocking<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&crate::db::repository::Repository) -> R + Send + 'static,
        R: Send + 'static,
    {
        let repo = Arc::clone(&self.repo);
        tokio::task::spawn_blocking(move || f(&repo))
            .await
            .expect("repo task panicked")
    }

    async fn get_torrent_session(&self) -> anyhow::Result<Arc<Session>> {
        let dir = self.download_dir().await;

        // Hold write lock for the entire check-and-create to prevent races
        let mut guard = self.torrent_session.write().await;
        if let Some((ref existing_dir, ref session)) = *guard {
            if *existing_dir == dir {
                return Ok(session.clone());
            }
        }

        let session = Session::new(dir.clone().into())
            .await
            .map_err(|e| anyhow::anyhow!("Failed to create torrent session: {}", e))?;

        *guard = Some((dir, session.clone()));
        Ok(session)
    }

    pub async fn add_download(&self, download: Download) {
        let dl = download.clone();
        if let Err(e) = self
            .repo_blocking(move |repo| repo.insert_download(&dl))
            .await
        {
            tracing::error!("Failed to persist download to DB: {}", e);
        }
        // Record in history
        let dl2 = download.clone();
        if let Err(e) = self
            .repo_blocking(move |repo| repo.insert_history(&dl2))
            .await
        {
            tracing::error!("Failed to record download in history: {}", e);
        }
        let mut downloads = self.downloads.write().await;
        downloads.insert(download.id.clone(), download);
    }

    pub async fn update_download(&self, download: &Download) {
        {
            let mut downloads = self.downloads.write().await;
            if let Some(d) = downloads.get_mut(&download.id) {
                *d = download.clone();
            }
        } // lock released before DB call
        let dl = download.clone();
        if let Err(e) = self
            .repo_blocking(move |repo| repo.update_download(&dl))
            .await
        {
            tracing::error!("Failed to update download in DB: {}", e);
        }
        // Update history on terminal status changes
        match download.status {
            DownloadStatus::Completed | DownloadStatus::Failed => {
                let dl2 = download.clone();
                if let Err(e) = self
                    .repo_blocking(move |repo| repo.update_history(&dl2))
                    .await
                {
                    tracing::error!("Failed to update history: {}", e);
                }
            }
            _ => {}
        }
    }

    pub async fn get_all(&self) -> Vec<Download> {
        let downloads = self.downloads.read().await;
        downloads.values().cloned().collect()
    }

    pub async fn remove(&self, id: &str) {
        // Cancel any running task first
        self.cancel_download(id).await;

        // Clean up torrent handle and delete from session
        let torrent_id = {
            let mut handles = self.torrent_handles.write().await;
            handles.remove(id)
        };
        if let Some(tid) = torrent_id {
            if let Ok(session) = self.get_torrent_session().await {
                let _ = session.delete(TorrentIdOrHash::Id(tid), false).await;
            }
        }

        let id_owned = id.to_string();
        if let Err(e) = self
            .repo_blocking(move |repo| repo.delete_download(&id_owned))
            .await
        {
            tracing::error!("Failed to delete download from DB: {}", e);
        }
        let mut downloads = self.downloads.write().await;
        downloads.remove(id);
    }

    pub async fn remove_with_files(&self, id: &str) {
        let download = {
            let downloads = self.downloads.read().await;
            downloads.get(id).cloned()
        };

        // Grab the torrent handle BEFORE cancelling — the monitor_torrent cancel handler
        // would otherwise remove it from the map and call session.delete(tid, false) first.
        let torrent_id = {
            let mut handles = self.torrent_handles.write().await;
            handles.remove(id)
        };

        // For torrents: tell librqbit to delete torrent + files. This stops the torrent
        // internally so file handles are released before manual cleanup.
        if let Some(tid) = torrent_id {
            if let Ok(session) = self.get_torrent_session().await {
                if let Err(e) = session.delete(TorrentIdOrHash::Id(tid), true).await {
                    tracing::warn!("librqbit delete failed: {}", e);
                }
            }
        }

        // Now cancel the monitor loop (it will see the token and exit;
        // the torrent is already deleted from the session so its delete call is a no-op).
        self.cancel_download(id).await;

        // Always do manual cleanup: librqbit only deletes individual tracked files,
        // leaving behind the torrent folder and any untracked content (partial files,
        // padding files, etc). remove_dir_all ensures a complete wipe.
        if let Some(ref d) = download {
            let path = std::path::Path::new(&d.save_path);

            let download_dir = self.download_dir().await;
            let safe = match path.canonicalize() {
                Ok(canonical) => canonical.starts_with(&download_dir),
                Err(_) => d.save_path.starts_with(&download_dir) && !d.save_path.contains(".."),
            };

            if safe && path.exists() {
                if path.is_dir() {
                    if let Err(e) = tokio::fs::remove_dir_all(path).await {
                        tracing::warn!("Failed to delete directory {}: {}", d.save_path, e);
                    }
                } else if let Err(e) = tokio::fs::remove_file(path).await {
                    tracing::warn!("Failed to delete file {}: {}", d.save_path, e);
                }
            } else if !safe {
                tracing::warn!(
                    "Refusing to delete file outside download directory: {}",
                    d.save_path
                );
            }
        }

        // Clean up from DB and in-memory state
        let id_owned = id.to_string();
        if let Err(e) = self
            .repo_blocking(move |repo| repo.delete_download(&id_owned))
            .await
        {
            tracing::error!("Failed to delete download from DB: {}", e);
        }
        let mut downloads = self.downloads.write().await;
        downloads.remove(id);
    }

    async fn register_cancel_token(&self, id: &str) -> CancellationToken {
        let token = CancellationToken::new();
        let mut tokens = self.cancel_tokens.write().await;
        // Cancel any previous token to prevent orphaned tasks
        if let Some(old) = tokens.insert(id.to_string(), token.clone()) {
            old.cancel();
        }
        token
    }

    pub async fn cancel_download(&self, id: &str) {
        let mut tokens = self.cancel_tokens.write().await;
        if let Some(token) = tokens.remove(id) {
            token.cancel();
        }
    }

    pub async fn pause_download(&self, id: &str) {
        // Check if this is a torrent with a live handle — use native pause
        let torrent_id = {
            let handles = self.torrent_handles.read().await;
            handles.get(id).copied()
        };

        if let Some(tid) = torrent_id {
            if let Ok(session) = self.get_torrent_session().await {
                if let Some(handle) = session.get(TorrentIdOrHash::Id(tid)) {
                    if let Err(e) = session.pause(&handle).await {
                        tracing::error!("Failed to pause torrent via librqbit: {}", e);
                    }
                }
            }
        } else {
            // HTTP download — cancel the token (no native pause support)
            self.cancel_download(id).await;
        }

        let download = {
            let mut downloads = self.downloads.write().await;
            if let Some(d) = downloads.get_mut(id) {
                if d.status == DownloadStatus::Downloading || d.status == DownloadStatus::Seeding {
                    d.status = DownloadStatus::Paused;
                    d.speed = 0;
                    d.upload_speed = 0;
                }
                Some(d.clone())
            } else {
                None
            }
        };
        if let Some(d) = download {
            if let Err(e) = self
                .repo_blocking(move |repo| repo.update_download(&d))
                .await
            {
                tracing::error!("Failed to persist pause state: {}", e);
            }
        }
    }

    pub async fn resume_download(self: &Arc<Self>, id: &str) {
        let download = {
            let downloads = self.downloads.read().await;
            downloads.get(id).cloned()
        };
        let Some(d) = download else { return };

        if d.status != DownloadStatus::Paused
            && d.status != DownloadStatus::Failed
            && d.status != DownloadStatus::Stopped
        {
            return;
        }

        // Try native librqbit unpause first (torrent was paused, not removed)
        let torrent_id = {
            let handles = self.torrent_handles.read().await;
            handles.get(id).copied()
        };

        if let Some(tid) = torrent_id {
            if let Ok(session) = self.get_torrent_session().await {
                if let Some(handle) = session.get(TorrentIdOrHash::Id(tid)) {
                    if handle.is_paused() {
                        if let Err(e) = session.unpause(&handle).await {
                            tracing::error!("Failed to unpause torrent: {}", e);
                            // Fall through to start_download as fallback
                        } else {
                            // Successfully unpaused — update status and re-monitor
                            {
                                let mut downloads = self.downloads.write().await;
                                if let Some(dl) = downloads.get_mut(id) {
                                    dl.status = DownloadStatus::Downloading;
                                    dl.error_message = None;
                                }
                            }
                            let mut d = d.clone();
                            d.status = DownloadStatus::Downloading;
                            d.error_message = None;
                            self.update_download(&d).await;

                            // Spawn a monitoring task
                            let state = Arc::clone(self);
                            let cancel_token = state.register_cancel_token(id).await;
                            let id = id.to_string();
                            tokio::spawn(async move {
                                state.monitor_torrent(id, tid, cancel_token).await;
                            });
                            return;
                        }
                    }
                }
            }
        }

        // Fallback: re-start from scratch (for HTTP, failed torrents, etc.)
        let state = Arc::clone(self);
        state.start_download(d).await;
    }

    pub async fn start_download(self: Arc<Self>, download: Download) {
        let state = Arc::clone(&self);
        let cancel_token = state.register_cancel_token(&download.id).await;

        tokio::spawn(async move {
            let protocol = crate::worker::detect_protocol(&download.url);
            let mut download = download;
            download.protocol = protocol.clone();
            download.status = DownloadStatus::Downloading;

            state.update_download(&download).await;

            match protocol {
                Protocol::Http => {
                    state
                        .clone()
                        .handle_http_download(download, cancel_token)
                        .await;
                }
                Protocol::Torrent => {
                    state
                        .clone()
                        .handle_torrent_download(download, cancel_token)
                        .await;
                }
                _ => {
                    let mut failed = download;
                    failed.status = DownloadStatus::Failed;
                    failed.error_message = Some("Protocol not yet supported".to_string());
                    state.update_download(&failed).await;
                }
            }
        });
    }

    async fn handle_http_download(
        self: Arc<Self>,
        download: Download,
        cancel_token: CancellationToken,
    ) {
        let max_conns = {
            let settings = self.settings.read().await;
            settings.max_connections_per_file as usize
        };

        let worker = HttpDownloader::new(download.clone(), max_conns, cancel_token.clone());
        let downloaded_atomic = Arc::clone(&worker.downloaded);
        let active_conns_atomic = Arc::clone(&worker.active_conns);
        let total_size_atomic = Arc::clone(&worker.total_size);
        let download_id = download.id.clone();

        // Spawn the actual download on a separate task
        let download_task = tokio::spawn(async move {
            let mut worker = worker;
            worker.run().await
        });

        // Monitor loop — poll atomics every second, persist to DB less often
        let mut last_downloaded: u64 = 0;
        let mut last_time = std::time::Instant::now();
        let mut db_tick: u32 = 0;

        loop {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;

            if download_task.is_finished() {
                break;
            }

            if cancel_token.is_cancelled() {
                download_task.abort();
                break;
            }

            let current_downloaded = downloaded_atomic.load(std::sync::atomic::Ordering::Relaxed);
            let current_conns = active_conns_atomic.load(std::sync::atomic::Ordering::Relaxed);
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

            // Update in-memory state (quick — just a HashMap write)
            let snapshot = {
                let mut downloads = self.downloads.write().await;
                if let Some(d) = downloads.get_mut(&download_id) {
                    d.downloaded_size = current_downloaded;
                    d.connections = current_conns;
                    d.speed = speed;
                    if current_total > 0 {
                        d.total_size = current_total;
                    }
                    if d.total_size > 0 {
                        d.progress = (current_downloaded as f64 / d.total_size as f64) * 100.0;
                        if speed > 0 {
                            let remaining = d.total_size.saturating_sub(current_downloaded);
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
                    Some(d.clone())
                } else {
                    None
                }
            }; // write lock released here

            // Persist to DB every 5 seconds (not every second)
            db_tick += 1;
            if db_tick >= 5 {
                db_tick = 0;
                if let Some(snap) = snapshot {
                    let repo = Arc::clone(&self.repo);
                    tokio::task::spawn_blocking(move || {
                        let _ = repo.update_download(&snap);
                    });
                }
            }
        }

        // Collect the result
        match download_task.await {
            Ok(Ok(result)) => {
                if cancel_token.is_cancelled() {
                    return;
                }

                let mut completed = result.download;
                completed.status = DownloadStatus::Completed;
                completed.progress = 100.0;
                if completed.total_size > 0 {
                    completed.downloaded_size = completed.total_size;
                }
                completed.speed = 0;
                completed.connections = 0;
                completed.eta = None;
                completed.completed_at = Some(chrono::Utc::now());
                self.update_download(&completed).await;

                // If the downloaded file is a .torrent, auto-start it
                if result.is_torrent_file {
                    tracing::info!(
                        "Detected .torrent file: {}, auto-starting torrent download",
                        completed.save_path
                    );

                    let torrent_path = completed.save_path.clone();
                    let original_id = completed.id.clone();

                    match tokio::fs::read(&torrent_path).await {
                        Ok(torrent_bytes) => {
                            let settings = self.settings.read().await;
                            let dir = settings.download_dir.clone();
                            drop(settings);

                            let raw_name = librqbit::torrent_from_bytes(&torrent_bytes)
                                .ok()
                                .and_then(|meta| {
                                    meta.info
                                        .name
                                        .as_ref()
                                        .map(|n: &librqbit::ByteBufOwned| n.to_string())
                                })
                                .unwrap_or_else(|| "torrent-download".to_string());
                            let name = crate::domain::sanitize_filename(&raw_name);

                            let mut torrent_download =
                                Download::new(format!("torrent://{}", name), &dir);
                            torrent_download.filename = name;
                            torrent_download.protocol = Protocol::Torrent;
                            torrent_download.status = DownloadStatus::Downloading;

                            let torrent_cancel =
                                self.register_cancel_token(&torrent_download.id).await;
                            self.add_download(torrent_download.clone()).await;

                            let add = librqbit::AddTorrent::from_bytes(torrent_bytes);
                            if let Err(e) = session_add_and_wait(
                                &self,
                                add,
                                &mut torrent_download,
                                &torrent_cancel,
                            )
                            .await
                            {
                                let current_status = {
                                    let downloads = self.downloads.read().await;
                                    downloads
                                        .get(&torrent_download.id)
                                        .map(|d| d.status.clone())
                                };
                                match current_status {
                                    Some(DownloadStatus::Paused)
                                    | Some(DownloadStatus::Stopped)
                                    | Some(DownloadStatus::Completed)
                                    | Some(DownloadStatus::Seeding) => {}
                                    _ => {
                                        torrent_download.status = DownloadStatus::Failed;
                                        torrent_download.error_message =
                                            Some(format!("Torrent failed: {}", e));
                                        self.update_download(&torrent_download).await;
                                    }
                                }
                            }

                            // Clean up the .torrent file
                            if let Err(e) = tokio::fs::remove_file(&torrent_path).await {
                                tracing::warn!("Failed to delete .torrent file: {}", e);
                            }

                            // Remove the original HTTP download entry
                            self.remove(&original_id).await;
                        }
                        Err(e) => {
                            tracing::error!("Failed to read .torrent file: {}", e);
                        }
                    }
                }
            }
            Ok(Err(e)) => {
                if !cancel_token.is_cancelled() {
                    let mut failed = download;
                    failed.status = DownloadStatus::Failed;
                    failed.error_message = Some(e.to_string());
                    failed.speed = 0;
                    failed.connections = 0;
                    self.update_download(&failed).await;
                }
            }
            Err(_) => {
                // Task was aborted (cancelled)
                if !cancel_token.is_cancelled() {
                    let mut failed = download;
                    failed.status = DownloadStatus::Failed;
                    failed.error_message = Some("Download task aborted".to_string());
                    failed.speed = 0;
                    failed.connections = 0;
                    self.update_download(&failed).await;
                }
            }
        }
    }

    /// Start a torrent download from raw .torrent file bytes.
    /// Used by the qBittorrent compat API for .torrent file uploads.
    pub async fn start_torrent_from_bytes(
        self: Arc<Self>,
        mut download: Download,
        torrent_bytes: Vec<u8>,
    ) {
        let state = Arc::clone(&self);
        let cancel_token = state.register_cancel_token(&download.id).await;

        tokio::spawn(async move {
            download.protocol = Protocol::Torrent;
            download.status = DownloadStatus::Downloading;
            state.update_download(&download).await;

            let add = librqbit::AddTorrent::from_bytes(torrent_bytes);
            if let Err(e) = session_add_and_wait(&state, add, &mut download, &cancel_token).await {
                let current_status = {
                    let downloads = state.downloads.read().await;
                    downloads.get(&download.id).map(|d| d.status.clone())
                };
                match current_status {
                    Some(DownloadStatus::Paused)
                    | Some(DownloadStatus::Stopped)
                    | Some(DownloadStatus::Completed)
                    | Some(DownloadStatus::Seeding) => {}
                    _ => {
                        download.status = DownloadStatus::Failed;
                        download.error_message = Some(format!("Torrent failed: {}", e));
                        state.update_download(&download).await;
                    }
                }
            }
        });
    }

    async fn handle_torrent_download(
        self: Arc<Self>,
        mut download: Download,
        cancel_token: CancellationToken,
    ) {
        let url = download.url.clone();
        let add = if url.starts_with("magnet:") {
            librqbit::AddTorrent::from_url(&url)
        } else {
            match reqwest::get(&url).await {
                Ok(resp) => {
                    let bytes = match resp.bytes().await {
                        Ok(b) => b,
                        Err(e) => {
                            download.status = DownloadStatus::Failed;
                            download.error_message = Some(e.to_string());
                            self.update_download(&download).await;
                            return;
                        }
                    };
                    librqbit::AddTorrent::from_bytes(bytes)
                }
                Err(e) => {
                    download.status = DownloadStatus::Failed;
                    download.error_message = Some(e.to_string());
                    self.update_download(&download).await;
                    return;
                }
            }
        };

        if let Err(e) = session_add_and_wait(&self, add, &mut download, &cancel_token).await {
            // Only mark as failed if not already handled (paused/stopped/completed)
            let current_status = {
                let downloads = self.downloads.read().await;
                downloads.get(&download.id).map(|d| d.status.clone())
            };
            match current_status {
                Some(DownloadStatus::Paused)
                | Some(DownloadStatus::Stopped)
                | Some(DownloadStatus::Completed)
                | Some(DownloadStatus::Seeding) => {
                    // Already handled by monitor_torrent
                }
                _ => {
                    download.status = DownloadStatus::Failed;
                    download.error_message = Some(format!("Torrent failed: {}", e));
                    self.update_download(&download).await;
                }
            }
        }
    }

    // ─── History ────────────────────────────────────────

    pub async fn get_all_history(&self) -> Vec<serde_json::Value> {
        self.repo_blocking(|repo| repo.get_all_history().unwrap_or_default())
            .await
    }

    pub async fn delete_history(&self, id: &str) {
        let id_owned = id.to_string();
        if let Err(e) = self
            .repo_blocking(move |repo| repo.delete_history(&id_owned))
            .await
        {
            tracing::error!("Failed to delete history entry: {}", e);
        }
    }

    pub async fn delete_all_history(&self) {
        if let Err(e) = self.repo_blocking(|repo| repo.delete_all_history()).await {
            tracing::error!("Failed to clear history: {}", e);
        }
    }
}

impl ManagerState {
    /// Monitor an existing torrent handle (used after adding or unpausing).
    /// Returns when torrent is paused, cancelled, or errors out.
    async fn monitor_torrent(
        self: Arc<Self>,
        download_id: String,
        torrent_id: usize,
        cancel_token: CancellationToken,
    ) {
        let session = match self.get_torrent_session().await {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("Failed to get torrent session for monitoring: {}", e);
                return;
            }
        };

        let handle = match session.get(TorrentIdOrHash::Id(torrent_id)) {
            Some(h) => h,
            None => {
                tracing::error!("Torrent handle {} not found in session", torrent_id);
                let snap = {
                    let mut downloads = self.downloads.write().await;
                    if let Some(d) = downloads.get_mut(&download_id) {
                        d.status = DownloadStatus::Failed;
                        d.error_message = Some("Torrent handle lost".to_string());
                        Some(d.clone())
                    } else {
                        None
                    }
                };
                if let Some(snap) = snap {
                    let repo = Arc::clone(&self.repo);
                    tokio::task::spawn_blocking(move || {
                        let _ = repo.update_download(&snap);
                    });
                }
                return;
            }
        };

        let mut db_tick: u32 = 0;

        loop {
            // Check if cancelled (stop/remove)
            if cancel_token.is_cancelled() {
                let current_status = {
                    let downloads = self.downloads.read().await;
                    downloads.get(&download_id).map(|d| d.status.clone())
                };
                match current_status {
                    Some(DownloadStatus::Paused) => {
                        return;
                    }
                    _ => {
                        let _ = session.delete(handle.id().into(), false).await;
                        {
                            let mut handles = self.torrent_handles.write().await;
                            handles.remove(&download_id);
                        }
                        // Don't set status here — let the caller (remove/cancel) handle final state.
                        // Just clean up speed so the UI doesn't show stale values.
                        {
                            let mut downloads = self.downloads.write().await;
                            if let Some(d) = downloads.get_mut(&download_id) {
                                d.speed = 0;
                                d.upload_speed = 0;
                            }
                        }
                        return;
                    }
                }
            }

            // Check if paused via librqbit
            if handle.is_paused() {
                let snap = {
                    let mut downloads = self.downloads.write().await;
                    if let Some(d) = downloads.get_mut(&download_id) {
                        d.status = DownloadStatus::Paused;
                        d.speed = 0;
                        d.upload_speed = 0;
                        Some(d.clone())
                    } else {
                        None
                    }
                };
                if let Some(snap) = snap {
                    let repo = Arc::clone(&self.repo);
                    tokio::task::spawn_blocking(move || {
                        let _ = repo.update_download(&snap);
                    });
                }
                return;
            }

            let stats = handle.stats();

            let snapshot = {
                let mut downloads = self.downloads.write().await;
                if let Some(download) = downloads.get_mut(&download_id) {
                    download.total_size = stats.total_bytes;
                    download.downloaded_size = stats.progress_bytes;
                    if stats.total_bytes > 0 {
                        download.progress =
                            (stats.progress_bytes as f64 / stats.total_bytes as f64) * 100.0;
                    }

                    if let Some(live) = &stats.live {
                        download.speed = (live.download_speed.mbps * 1_048_576.0) as u64;
                        download.upload_speed = (live.upload_speed.mbps * 1_048_576.0) as u64;
                        let ps = &live.snapshot.peer_stats;
                        download.peers = ps.live as u32;
                        download.seeds = ps.seen as u32;
                        download.eta = live.time_remaining.as_ref().map(|t| format!("{}", t));
                    }

                    if stats.finished && download.status != DownloadStatus::Seeding {
                        download.status = DownloadStatus::Seeding;
                        download.speed = 0;
                        download.progress = 100.0;
                        download.completed_at = Some(chrono::Utc::now());
                        download.eta = None;
                    }

                    Some(download.clone())
                } else {
                    None
                }
            }; // write lock released

            // DB write every 5 ticks (seeding: every 5*2=10s, downloading: every 5s)
            db_tick += 1;
            if db_tick >= 5 {
                db_tick = 0;
                if let Some(snap) = snapshot {
                    let repo = Arc::clone(&self.repo);
                    tokio::task::spawn_blocking(move || {
                        let _ = repo.update_download(&snap);
                    });
                }
            }

            let sleep_dur = if stats.finished { 2 } else { 1 };
            tokio::time::sleep(std::time::Duration::from_secs(sleep_dur)).await;
        }
    }
}

async fn session_add_and_wait(
    state: &Arc<ManagerState>,
    add: librqbit::AddTorrent<'_>,
    download: &mut Download,
    cancel_token: &CancellationToken,
) -> anyhow::Result<()> {
    let session = state.get_torrent_session().await?;

    let download_dir = state.download_dir().await;
    let opts = librqbit::AddTorrentOptions {
        overwrite: true,
        ..Default::default()
    };

    let handle = session
        .add_torrent(add, Some(opts))
        .await
        .map_err(|e| anyhow::anyhow!("Failed to add torrent: {}", e))?
        .into_handle()
        .ok_or_else(|| anyhow::anyhow!("Torrent was a duplicate or couldn't get handle"))?;

    let torrent_id = handle.id();

    // Store the handle mapping for pause/resume
    {
        let mut handles = state.torrent_handles.write().await;
        handles.insert(download.id.clone(), torrent_id);
    }

    // Update name, save_path, content_path, and info_hash from torrent metadata
    if let Some(raw_name) = handle.name() {
        let name = crate::domain::sanitize_filename(&raw_name);
        download.filename = name.clone();
        let new_path = format!("{}/{}", download_dir.trim_end_matches('/'), name);
        download.save_path = new_path.clone();
        download.content_path = Some(new_path);
    }
    let hash_str = handle.info_hash().as_string();
    if download.info_hash.is_none() {
        download.info_hash = Some(hash_str);
    }

    state.update_download(download).await;

    // Use the shared monitor
    let state_clone = Arc::clone(state);
    let dl_id = download.id.clone();
    let ct = cancel_token.clone();
    state_clone.monitor_torrent(dl_id, torrent_id, ct).await;

    // After monitoring ends, read back the latest status
    let latest = {
        let downloads = state.downloads.read().await;
        downloads.get(&download.id).cloned()
    };
    if let Some(latest) = latest {
        *download = latest;
    }

    Ok(())
}

pub type SharedState = Arc<ManagerState>;
