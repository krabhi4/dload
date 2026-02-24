use crate::domain::{Download, DownloadStatus, Protocol, Settings};
use crate::worker::http::HttpDownloader;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};

#[derive(Clone)]
pub struct ManagerState {
    pub downloads: Arc<RwLock<HashMap<String, Download>>>,
    pub settings: Arc<RwLock<Settings>>,
    pub tx: broadcast::Sender<DownloadEvent>,
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
        
        Self {
            downloads: Arc::new(RwLock::new(HashMap::new())),
            settings: Arc::new(RwLock::new(settings)),
            tx,
        }
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
        let chunk_size = settings.chunk_size as usize;
        drop(settings);
        
        let state = Arc::clone(&self);
        
        tokio::spawn(async move {
            let protocol = crate::worker::detect_protocol(&download.url);
            let mut download = download;
            download.protocol = protocol.clone();
            download.status = DownloadStatus::Downloading;
            
            state.update_download(&download).await;
            
            let result = match protocol {
                Protocol::Http => {
                    let mut worker = HttpDownloader::new(download.clone(), chunk_size);
                    worker.run().await
                }
                _ => {
                    Err(anyhow::anyhow!("Protocol not yet supported"))
                }
            };
            
            match result {
                Ok(mut completed) => {
                    completed.status = DownloadStatus::Completed;
                    state.update_download(&completed).await;
                }
                Err(e) => {
                    let mut failed = download;
                    failed.status = DownloadStatus::Failed;
                    failed.error_message = Some(e.to_string());
                    state.update_download(&failed).await;
                }
            }
        });
    }
}

pub type SharedState = Arc<ManagerState>;
