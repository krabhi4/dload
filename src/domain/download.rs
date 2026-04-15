use chrono::{DateTime, Utc};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

static RE_CONTROL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[\x00-\x1F\x7F]").expect("control regex"));
static RE_BIDI: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[\u{202A}-\u{202E}\u{2066}-\u{2069}]").expect("bidi regex"));
static RE_WIN_ILLEGAL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"[<>:"|?*]"#).expect("win-illegal regex"));
static RE_TRAILING_DOTS_SPACES: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[. ]+$").expect("trailing dots/spaces regex"));
static RE_RESERVED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(con|prn|aux|nul|com[1-9]|lpt[1-9])(\..+)?$").expect("reserved regex")
});

/// Strip path traversal, null bytes, and dangerous characters from filenames.
pub fn sanitize_filename(name: &str) -> String {
    let decoded = urlencoding::decode(name).unwrap_or(std::borrow::Cow::Borrowed(name));

    let base: &str = decoded
        .rsplit('/')
        .next()
        .unwrap_or(&decoded)
        .rsplit('\\')
        .next()
        .unwrap_or(&decoded);

    // Preserve a single leading dot (e.g. `.gitignore`) but treat `..` etc. as
    // path-traversal residue to be stripped in step 8.
    let had_single_leading_dot = base.starts_with('.') && !base.starts_with("..");

    let s = RE_CONTROL.replace_all(base, "");
    let s = RE_BIDI.replace_all(&s, "");
    let s = RE_WIN_ILLEGAL.replace_all(&s, "_");
    let s = s.trim();
    let s = RE_TRAILING_DOTS_SPACES.replace_all(s, "").to_string();

    let s = if had_single_leading_dot {
        s
    } else {
        s.trim_start_matches('.').to_string()
    };

    if s.is_empty() || s == "." || s == ".." {
        return format!("download-{}", uuid::Uuid::new_v4());
    }

    if RE_RESERVED.is_match(&s) {
        return format!("_{}", s);
    }

    s
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
    pub http_mirror_status: Option<String>,
    pub http_mirror_url: Option<String>,
    /// Persisted flag: true when download should auto-resume after app restart.
    /// Set true on start/resume, false on user-initiated pause or completion.
    #[serde(default)]
    pub restart_resume: bool,
    /// Ordering position for drag-and-drop reordering. Lower = higher priority.
    #[serde(default)]
    pub position: i32,
}

impl Download {
    pub fn new(url: String, save_dir: &str) -> Self {
        let raw_filename = url.split('/').next_back().unwrap_or("download").to_string();
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
            http_mirror_status: None,
            http_mirror_url: None,
            restart_resume: false,
            position: 0,
        }
    }
}
