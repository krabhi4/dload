use crate::worker::http::{probe, Transfer};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// Downloads a file via HTTP to a fixed path without mutating any Download fields.
pub struct MirrorDownloader {
    url: String,
    target_path: String,
    cancel_token: CancellationToken,
    pub downloaded: Arc<AtomicU64>,
    pub total_size: Arc<AtomicU64>,
}

pub struct MirrorResult {
    pub is_zip: bool,
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
        crate::worker::check_target(&self.url)?;
        let client = crate::worker::http_client()?;
        let probe = tokio::select! {
            r = probe(&client, &self.url) => r?,
            _ = self.cancel_token.cancelled() => anyhow::bail!("Mirror download cancelled"),
        };
        let is_zip = probe.content_type.contains("application/zip")
            || self.url.to_lowercase().ends_with(".zip");
        self.total_size
            .store(probe.total.unwrap_or(0), Ordering::Relaxed);

        let file = tokio::fs::File::create(&self.target_path).await?;
        let transfer = Transfer {
            client,
            url: self.url.clone(),
            max_conns: 4,
            min_split: 20 * 1024 * 1024,
            downloaded: Arc::clone(&self.downloaded),
            active_conns: Arc::new(AtomicU32::new(0)),
            cancel: self.cancel_token.clone(),
        };
        transfer
            .run(file.into_std().await, probe, None, None)
            .await?;

        Ok(MirrorResult { is_zip })
    }
}

/// Extract a zip file to a target directory with path traversal protection.
/// Returns the list of extracted file paths (relative to target_dir).
///
/// Safety: rejects symlinks, path traversal, and limits total extracted size to 50 GB.
pub fn extract_zip_safe(zip_path: &str, target_dir: &str) -> anyhow::Result<Vec<String>> {
    const MAX_EXTRACT_SIZE: u64 = 50 * 1024 * 1024 * 1024; // 50 GB

    let file = std::fs::File::open(zip_path)?;
    let mut archive = zip::ZipArchive::new(file)?;
    let target = std::path::Path::new(target_dir);

    let mut extracted = Vec::new();
    let mut total_extracted: u64 = 0;

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;

        // Reject symlinks — could escape target directory via indirection
        if entry.is_symlink() {
            tracing::warn!("Skipping symlink zip entry: {:?}", entry.name());
            continue;
        }

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
        if !out_path.starts_with(target) {
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
            let written = std::io::copy(&mut entry, &mut outfile)?;
            total_extracted += written;
            if total_extracted > MAX_EXTRACT_SIZE {
                anyhow::bail!(
                    "Zip extraction exceeded maximum size limit ({} bytes extracted)",
                    total_extracted
                );
            }
            extracted.push(entry_path.to_string_lossy().to_string());
        }
    }

    Ok(extracted)
}
