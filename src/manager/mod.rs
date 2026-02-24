use crate::db::repository::Repository;
use crate::domain::{Download, DownloadStatus, Protocol, Settings};
use crate::worker::http::HttpDownloader;
use librqbit::api::TorrentIdOrHash;
use librqbit::Session;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{RwLock, OnceCell};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct ManagerState {
    pub downloads: Arc<RwLock<HashMap<String, Download>>>,
    pub settings: Arc<RwLock<Settings>>,
    pub repo: Arc<Repository>,
    torrent_session: Arc<OnceCell<Arc<Session>>>,
    download_dir: String,
    cancel_tokens: Arc<RwLock<HashMap<String, CancellationToken>>>,
    /// Maps download ID -> librqbit torrent handle ID for pause/resume
    torrent_handles: Arc<RwLock<HashMap<String, usize>>>,
}

impl ManagerState {
    pub fn new(settings: Settings, repo: Arc<Repository>) -> Self {
        let download_dir = settings.download_dir.clone();

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
            torrent_session: Arc::new(OnceCell::new()),
            download_dir,
            cancel_tokens: Arc::new(RwLock::new(HashMap::new())),
            torrent_handles: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    async fn get_torrent_session(&self) -> anyhow::Result<Arc<Session>> {
        let dir = self.download_dir.clone();
        let session = self.torrent_session.get_or_try_init(|| async {
            Session::new(dir.into())
                .await
                .map_err(|e| anyhow::anyhow!("Failed to create torrent session: {}", e))
        }).await?;
        Ok(session.clone())
    }

    pub async fn add_download(&self, download: Download) {
        if let Err(e) = self.repo.insert_download(&download) {
            tracing::error!("Failed to persist download to DB: {}", e);
        }
        let mut downloads = self.downloads.write().await;
        downloads.insert(download.id.clone(), download);
    }

    pub async fn update_download(&self, download: &Download) {
        if let Err(e) = self.repo.update_download(download) {
            tracing::error!("Failed to update download in DB: {}", e);
        }
        let mut downloads = self.downloads.write().await;
        if let Some(d) = downloads.get_mut(&download.id) {
            *d = download.clone();
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

        if let Err(e) = self.repo.delete_download(id) {
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

        if let Some(d) = download {
            let path = std::path::Path::new(&d.save_path);
            if path.exists() {
                if path.is_dir() {
                    if let Err(e) = tokio::fs::remove_dir_all(path).await {
                        tracing::warn!("Failed to delete directory {}: {}", d.save_path, e);
                    }
                } else if let Err(e) = tokio::fs::remove_file(path).await {
                    tracing::warn!("Failed to delete file {}: {}", d.save_path, e);
                }
            }
        }

        self.remove(id).await;
    }

    async fn register_cancel_token(&self, id: &str) -> CancellationToken {
        let token = CancellationToken::new();
        let mut tokens = self.cancel_tokens.write().await;
        tokens.insert(id.to_string(), token.clone());
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
            if let Err(e) = self.repo.update_download(&d) {
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

        if d.status != DownloadStatus::Paused && d.status != DownloadStatus::Failed && d.status != DownloadStatus::Stopped {
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
                    state.clone().handle_http_download(download, cancel_token).await;
                }
                Protocol::Torrent => {
                    state.clone().handle_torrent_download(download, cancel_token).await;
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

    async fn handle_http_download(self: Arc<Self>, download: Download, cancel_token: CancellationToken) {
        let mut worker = HttpDownloader::new(download.clone());
        match worker.run().await {
            Ok(result) => {
                if cancel_token.is_cancelled() {
                    return;
                }

                let mut completed = result.download;
                completed.status = DownloadStatus::Completed;
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

                            let name = librqbit::torrent_from_bytes(&torrent_bytes)
                                .ok()
                                .and_then(|meta| {
                                    meta.info.name.as_ref().map(|n: &librqbit::ByteBufOwned| n.to_string())
                                })
                                .unwrap_or_else(|| "torrent-download".to_string());

                            let mut torrent_download = Download::new(
                                format!("torrent://{}", name),
                                &dir,
                            );
                            torrent_download.filename = name;
                            torrent_download.protocol = Protocol::Torrent;
                            torrent_download.status = DownloadStatus::Downloading;

                            let torrent_cancel = self.register_cancel_token(&torrent_download.id).await;
                            self.add_download(torrent_download.clone()).await;

                            match self.get_torrent_session().await {
                                Ok(_session) => {
                                    let add = librqbit::AddTorrent::from_bytes(torrent_bytes);
                                    if let Err(e) = session_add_and_wait(
                                        &self, add, &mut torrent_download, &torrent_cancel
                                    ).await {
                                        let current_status = {
                                            let downloads = self.downloads.read().await;
                                            downloads.get(&torrent_download.id).map(|d| d.status.clone())
                                        };
                                        match current_status {
                                            Some(DownloadStatus::Paused) | Some(DownloadStatus::Stopped) | Some(DownloadStatus::Completed) | Some(DownloadStatus::Seeding) => {}
                                            _ => {
                                                torrent_download.status = DownloadStatus::Failed;
                                                torrent_download.error_message = Some(format!("Torrent failed: {}", e));
                                                self.update_download(&torrent_download).await;
                                            }
                                        }
                                    }
                                }
                                Err(e) => {
                                    torrent_download.status = DownloadStatus::Failed;
                                    torrent_download.error_message =
                                        Some(format!("Failed to init torrent session: {}", e));
                                    self.update_download(&torrent_download).await;
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
            Err(e) => {
                if !cancel_token.is_cancelled() {
                    let mut failed = download;
                    failed.status = DownloadStatus::Failed;
                    failed.error_message = Some(e.to_string());
                    failed.speed = 0;
                    self.update_download(&failed).await;
                }
            }
        }
    }

    async fn handle_torrent_download(self: Arc<Self>, mut download: Download, cancel_token: CancellationToken) {
        match self.get_torrent_session().await {
            Ok(_session) => {
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
                        Some(DownloadStatus::Paused) | Some(DownloadStatus::Stopped) | Some(DownloadStatus::Completed) | Some(DownloadStatus::Seeding) => {
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
            Err(e) => {
                download.status = DownloadStatus::Failed;
                download.error_message = Some(format!("Failed to init torrent session: {}", e));
                self.update_download(&download).await;
            }
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
                let mut downloads = self.downloads.write().await;
                if let Some(d) = downloads.get_mut(&download_id) {
                    d.status = DownloadStatus::Failed;
                    d.error_message = Some("Torrent handle lost".to_string());
                    let _ = self.repo.update_download(d);
                }
                return;
            }
        };

        loop {
            // Check if cancelled (stop/remove)
            if cancel_token.is_cancelled() {
                // Check if this was a pause (don't delete from session)
                let current_status = {
                    let downloads = self.downloads.read().await;
                    downloads.get(&download_id).map(|d| d.status.clone())
                };
                match current_status {
                    Some(DownloadStatus::Paused) => {
                        // Paused via native librqbit pause — don't delete, just stop monitoring
                        return;
                    }
                    _ => {
                        // Actually stopped/removed — delete from session
                        let _ = session.delete(handle.id().into(), false).await;
                        {
                            let mut handles = self.torrent_handles.write().await;
                            handles.remove(&download_id);
                        }

                        let current_status = {
                            let downloads = self.downloads.read().await;
                            downloads.get(&download_id).map(|d| d.status.clone())
                        };
                        match current_status {
                            Some(DownloadStatus::Stopped) => {}
                            _ => {
                                // Stopped seeding — mark completed
                                let mut downloads = self.downloads.write().await;
                                if let Some(d) = downloads.get_mut(&download_id) {
                                    d.status = DownloadStatus::Completed;
                                    d.progress = 100.0;
                                    d.speed = 0;
                                    d.upload_speed = 0;
                                    let _ = self.repo.update_download(d);
                                }
                            }
                        }
                        return;
                    }
                }
            }

            // Check if paused via librqbit
            if handle.is_paused() {
                // Update status and stop monitoring
                let mut downloads = self.downloads.write().await;
                if let Some(d) = downloads.get_mut(&download_id) {
                    d.status = DownloadStatus::Paused;
                    d.speed = 0;
                    d.upload_speed = 0;
                    let _ = self.repo.update_download(d);
                }
                return;
            }

            let stats = handle.stats();

            {
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
                    }

                    let _ = self.repo.update_download(download);
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

    // Update name and save_path from torrent metadata
    if let Some(name) = handle.name() {
        download.filename = name.clone();
        download.save_path = format!("{}/{}", state.download_dir, name);
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
