use crate::domain::{Download, DownloadStatus, Protocol, Settings};
use crate::worker::http::HttpDownloader;
use librqbit::Session;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock, OnceCell};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct ManagerState {
    pub downloads: Arc<RwLock<HashMap<String, Download>>>,
    pub settings: Arc<RwLock<Settings>>,
    pub tx: broadcast::Sender<DownloadEvent>,
    torrent_session: Arc<OnceCell<Arc<Session>>>,
    download_dir: String,
    cancel_tokens: Arc<RwLock<HashMap<String, CancellationToken>>>,
}

#[derive(Debug, Clone)]
pub enum DownloadEvent {
    Progress(Download),
    Completed(String),
    Failed(String, String),
}

impl ManagerState {
    pub fn new(settings: Settings) -> Self {
        let (tx, _) = broadcast::channel(100);
        let download_dir = settings.download_dir.clone();

        Self {
            downloads: Arc::new(RwLock::new(HashMap::new())),
            settings: Arc::new(RwLock::new(settings)),
            tx,
            torrent_session: Arc::new(OnceCell::new()),
            download_dir,
            cancel_tokens: Arc::new(RwLock::new(HashMap::new())),
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

    pub fn subscribe(&self) -> broadcast::Receiver<DownloadEvent> {
        self.tx.subscribe()
    }

    pub async fn add_download(&self, download: Download) {
        let mut downloads = self.downloads.write().await;
        downloads.insert(download.id.clone(), download.clone());
        let _ = self.tx.send(DownloadEvent::Progress(download));
    }

    pub async fn update_download(&self, download: &Download) {
        let mut downloads = self.downloads.write().await;
        if let Some(d) = downloads.get_mut(&download.id) {
            *d = download.clone();
        }
        let _ = self.tx.send(DownloadEvent::Progress(download.clone()));
    }

    pub async fn get_all(&self) -> Vec<Download> {
        let downloads = self.downloads.read().await;
        downloads.values().cloned().collect()
    }

    pub async fn remove(&self, id: &str) {
        // Cancel any running task first
        self.cancel_download(id).await;
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
        self.cancel_download(id).await;
        let mut downloads = self.downloads.write().await;
        if let Some(d) = downloads.get_mut(id) {
            if d.status == DownloadStatus::Downloading {
                d.status = DownloadStatus::Paused;
                d.speed = 0;
                d.upload_speed = 0;
                let _ = self.tx.send(DownloadEvent::Progress(d.clone()));
            }
        }
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
                                    match session_add_and_wait(
                                        &self, add, &mut torrent_download, &torrent_cancel
                                    ).await {
                                        Ok(()) => {
                                            torrent_download.status = DownloadStatus::Completed;
                                            torrent_download.progress = 100.0;
                                            torrent_download.speed = 0;
                                            torrent_download.upload_speed = 0;
                                            self.update_download(&torrent_download).await;
                                        }
                                        Err(e) => {
                                            if !torrent_cancel.is_cancelled() {
                                                torrent_download.status = DownloadStatus::Failed;
                                                torrent_download.error_message = Some(e.to_string());
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

                match session_add_and_wait(&self, add, &mut download, &cancel_token).await {
                    Ok(()) => {
                        download.status = DownloadStatus::Completed;
                        download.progress = 100.0;
                        download.speed = 0;
                        download.upload_speed = 0;
                        self.update_download(&download).await;
                    }
                    Err(e) => {
                        if !cancel_token.is_cancelled() {
                            download.status = DownloadStatus::Failed;
                            download.error_message = Some(e.to_string());
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

async fn session_add_and_wait(
    state: &ManagerState,
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

    // Update name and save_path from torrent metadata
    if let Some(name) = handle.name() {
        download.filename = name.clone();
        download.save_path = format!("{}/{}", state.download_dir, name);
    }

    // Poll progress until finished or cancelled
    loop {
        if cancel_token.is_cancelled() {
            // Try to delete the torrent from the session
            let _ = session.delete(
                handle.id().into(),
                false,
            ).await;
            return Err(anyhow::anyhow!("Download cancelled"));
        }

        let stats = handle.stats();

        download.total_size = stats.total_bytes;
        download.downloaded_size = stats.progress_bytes;
        if stats.total_bytes > 0 {
            download.progress =
                (stats.progress_bytes as f64 / stats.total_bytes as f64) * 100.0;
        }

        // Get live stats: speed, peers, ETA
        if let Some(live) = &stats.live {
            // mbps is actually MiB/s in librqbit — convert to bytes/s
            download.speed = (live.download_speed.mbps * 1_048_576.0) as u64;
            download.upload_speed = (live.upload_speed.mbps * 1_048_576.0) as u64;

            // Peer stats
            let ps = &live.snapshot.peer_stats;
            download.peers = ps.live as u32;
            download.seeds = ps.seen as u32;

            // ETA
            download.eta = live.time_remaining.as_ref().map(|t| format!("{}", t));
        }

        state.update_download(download).await;

        if stats.finished {
            download.speed = 0;
            download.upload_speed = 0;
            break;
        }

        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }

    Ok(())
}

pub type SharedState = Arc<ManagerState>;
