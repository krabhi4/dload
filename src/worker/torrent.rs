use crate::domain::{Download, DownloadStatus};
use librqbit::{AddTorrent, Session};
use std::sync::Arc;

pub struct TorrentDownloader {
    download: Download,
    session: Arc<Session>,
}

impl TorrentDownloader {
    pub fn new(download: Download, session: Arc<Session>) -> Self {
        Self { download, session }
    }

    pub async fn run(&mut self) -> anyhow::Result<Download> {
        let add_torrent = if self.download.url.starts_with("magnet:") {
            AddTorrent::from_url(&self.download.url)
        } else if std::path::Path::new(&self.download.url).exists() {
            // Local .torrent file path
            AddTorrent::from_local_filename(&self.download.url)?
        } else {
            // URL to a .torrent file — download it first
            let data = reqwest::get(&self.download.url)
                .await?
                .bytes()
                .await?;
            AddTorrent::from_bytes(data)
        };

        let handle = self.session
            .add_torrent(add_torrent, None)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to add torrent: {}", e))?
            .into_handle()
            .ok_or_else(|| anyhow::anyhow!("Torrent was a duplicate or couldn't get handle"))?;

        // Wait for completion
        handle.wait_until_completed().await
            .map_err(|e| anyhow::anyhow!("Torrent download failed: {}", e))?;

        self.download.status = DownloadStatus::Completed;
        self.download.progress = 100.0;
        Ok(self.download.clone())
    }
}
