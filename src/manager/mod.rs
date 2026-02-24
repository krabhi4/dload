use crate::domain::{Download, DownloadStatus, Protocol, Settings};
use crate::worker::http::HttpDownloader;
use librqbit::Session;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock, OnceCell};

#[derive(Clone)]
pub struct ManagerState {
    pub downloads: Arc<RwLock<HashMap<String, Download>>>,
    pub settings: Arc<RwLock<Settings>>,
    pub tx: broadcast::Sender<DownloadEvent>,
    torrent_session: Arc<OnceCell<Arc<Session>>>,
    download_dir: String,
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
        let mut downloads = self.downloads.write().await;
        downloads.remove(id);
    }

    pub async fn start_download(self: Arc<Self>, download: Download) {
        let settings = self.settings.read().await;
        let _chunk_size = settings.chunk_size as usize;
        drop(settings);

        let state = Arc::clone(&self);

        tokio::spawn(async move {
            let protocol = crate::worker::detect_protocol(&download.url);
            let mut download = download;
            download.protocol = protocol.clone();
            download.status = DownloadStatus::Downloading;

            state.update_download(&download).await;

            match protocol {
                Protocol::Http => {
                    state.clone().handle_http_download(download).await;
                }
                Protocol::Torrent => {
                    state.clone().handle_torrent_download(download).await;
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

    async fn handle_http_download(self: Arc<Self>, download: Download) {
        let mut worker = HttpDownloader::new(download.clone());
        match worker.run().await {
            Ok(result) => {
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

                    // Read the torrent file
                    match tokio::fs::read(&torrent_path).await {
                        Ok(torrent_bytes) => {
                            // Create a new download for the torrent content
                            let settings = self.settings.read().await;
                            let dir = settings.download_dir.clone();
                            drop(settings);

                            // Try to extract name from torrent metadata
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

                            self.add_download(torrent_download.clone()).await;

                            // Start torrent download
                            match self.get_torrent_session().await {
                                Ok(_session) => {
                                    let add = librqbit::AddTorrent::from_bytes(torrent_bytes);
                                    match session_add_and_wait(
                                        &self, add, &mut torrent_download
                                    ).await {
                                        Ok(()) => {
                                            torrent_download.status = DownloadStatus::Completed;
                                            torrent_download.progress = 100.0;
                                            self.update_download(&torrent_download).await;
                                        }
                                        Err(e) => {
                                            torrent_download.status = DownloadStatus::Failed;
                                            torrent_download.error_message = Some(e.to_string());
                                            self.update_download(&torrent_download).await;
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
                let mut failed = download;
                failed.status = DownloadStatus::Failed;
                failed.error_message = Some(e.to_string());
                failed.speed = 0;
                self.update_download(&failed).await;
            }
        }
    }

    async fn handle_torrent_download(self: Arc<Self>, mut download: Download) {
        match self.get_torrent_session().await {
            Ok(_session) => {
                let url = download.url.clone();
                let add = if url.starts_with("magnet:") {
                    librqbit::AddTorrent::from_url(&url)
                } else {
                    // URL to .torrent file
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

                match session_add_and_wait(&self, add, &mut download).await {
                    Ok(()) => {
                        download.status = DownloadStatus::Completed;
                        download.progress = 100.0;
                        self.update_download(&download).await;
                    }
                    Err(e) => {
                        download.status = DownloadStatus::Failed;
                        download.error_message = Some(e.to_string());
                        self.update_download(&download).await;
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
    _state: &ManagerState,
    add: librqbit::AddTorrent<'_>,
    _download: &mut Download,
) -> anyhow::Result<()> {
    let session = _state.get_torrent_session().await?;
    let handle = session
        .add_torrent(add, None)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to add torrent: {}", e))?
        .into_handle()
        .ok_or_else(|| anyhow::anyhow!("Torrent was a duplicate or couldn't get handle"))?;

    handle
        .wait_until_completed()
        .await
        .map_err(|e| anyhow::anyhow!("Torrent download failed: {}", e))?;

    Ok(())
}

pub type SharedState = Arc<ManagerState>;
