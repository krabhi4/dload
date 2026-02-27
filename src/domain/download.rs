use serde::{Deserialize, Serialize};
use chrono::{DateTime, Utc};

/// Strip path traversal, null bytes, and dangerous characters from filenames.
pub fn sanitize_filename(name: &str) -> String {
    // URL-decode first (in case of %2F etc.)
    let decoded = urlencoding::decode(name).unwrap_or(std::borrow::Cow::Borrowed(name));

    // Take only the last path component (strips ../ and absolute paths)
    let base = decoded
        .rsplit('/')
        .next()
        .unwrap_or(&decoded)
        .rsplit('\\')
        .next()
        .unwrap_or(&decoded);

    // Remove null bytes, control characters
    let cleaned: String = base
        .chars()
        .filter(|c| !c.is_control() && *c != '\0')
        .collect();

    // Remove leading dots (hidden files) and any remaining ..
    let cleaned = cleaned.trim_start_matches('.').replace("..", "");

    if cleaned.is_empty() {
        format!("download-{}", uuid::Uuid::new_v4())
    } else {
        cleaned
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum DownloadStatus {
    Queued,
    Downloading,
    Paused,
    Completed,
    Failed,
    Stopped,
    Seeding,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Protocol {
    Http,
    Ftp,
    Sftp,
    Torrent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Download {
    pub id: String,
    pub url: String,
    pub filename: String,
    pub save_path: String,
    pub total_size: u64,
    pub downloaded_size: u64,
    pub speed: u64,
    pub upload_speed: u64,
    pub progress: f64,
    pub status: DownloadStatus,
    pub protocol: Protocol,
    pub connections: u32,
    pub peers: u32,
    pub seeds: u32,
    pub eta: Option<String>,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub error_message: Option<String>,
    pub info_hash: Option<String>,
    pub category: Option<String>,
    pub content_path: Option<String>,
    /// Per-download speed limit in bytes/s; -1 = unlimited (mirrors qBit dl_limit)
    pub dl_limit: i64,
    /// Per-download upload limit in bytes/s; -1 = unlimited (mirrors qBit up_limit)
    pub up_limit: i64,
    pub sequential_download: bool,
    pub first_last_piece_prio: bool,
    /// JSON map of file index (string) → priority (u32); None = no per-file priorities set
    pub file_priorities_json: Option<String>,
}

impl Download {
    pub fn new(url: String, save_dir: &str) -> Self {
        let raw_filename = url
            .split('/')
            .next_back()
            .unwrap_or("download")
            .to_string();
        let filename = sanitize_filename(&raw_filename);

        Self {
            id: uuid::Uuid::new_v4().to_string(),
            url,
            filename: filename.clone(),
            save_path: format!("{}/{}", save_dir.trim_end_matches('/'), filename),
            total_size: 0,
            downloaded_size: 0,
            speed: 0,
            upload_speed: 0,
            progress: 0.0,
            status: DownloadStatus::Queued,
            protocol: Protocol::Http,
            connections: 1,
            peers: 0,
            seeds: 0,
            eta: None,
            created_at: Utc::now(),
            completed_at: None,
            error_message: None,
            info_hash: None,
            category: None,
            content_path: None,
            dl_limit: -1,
            up_limit: -1,
            sequential_download: false,
            first_last_piece_prio: false,
            file_priorities_json: None,
        }
    }
}
