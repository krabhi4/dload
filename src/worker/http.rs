use crate::domain::{Download, DownloadStatus};
use futures::stream::StreamExt;
use tokio::io::AsyncWriteExt;

pub struct HttpDownloader {
    download: Download,
    chunk_size: usize,
}

impl HttpDownloader {
    pub fn new(download: Download, chunk_size: usize) -> Self {
        Self { download, chunk_size }
    }

    pub async fn run(&mut self) -> anyhow::Result<Download> {
        let client = reqwest::Client::new();
        let response = client.get(&self.download.url).send().await?;
        
        let total_size: u64 = response
            .headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        
        self.download.total_size = total_size;

        let mut file = tokio::fs::File::create(&self.download.save_path).await?;
        let mut stream = response.bytes_stream();
        let mut downloaded: u64 = 0;
        let start_time = std::time::Instant::now();
        
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            file.write_all(&chunk).await?;
            downloaded += chunk.len() as u64;
            
            self.download.downloaded_size = downloaded;
            if self.download.total_size > 0 {
                self.download.progress = (downloaded as f64 / self.download.total_size as f64) * 100.0;
            }
            
            let elapsed = start_time.elapsed().as_secs_f64();
            if elapsed > 0.0 {
                self.download.speed = (downloaded as f64 / elapsed) as u64;
            }
        }
        
        file.flush().await?;
        self.download.status = DownloadStatus::Completed;
        
        Ok(self.download.clone())
    }
}
