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

/// Derive a starting filename from a URL. For HTTP-style URLs, use the last
/// path segment. For magnet URIs, prefer the `dn=` display-name query param so
/// we don't end up naming files after the last tracker's `/announce` path.
/// The real name is refreshed later from torrent metadata or Content-Disposition.
fn derive_initial_filename(url: &str) -> String {
    if url.starts_with("magnet:") {
        for pair in url.trim_start_matches("magnet:?").split('&') {
            if let Some(v) = pair.strip_prefix("dn=") {
                let decoded = urlencoding::decode(v)
                    .map(|cow| cow.into_owned())
                    .unwrap_or_else(|_| v.to_string());
                let decoded = decoded.replace('+', " ");
                if !decoded.is_empty() {
                    return decoded;
                }
            }
        }
        return "torrent-download".to_string();
    }
    url.split('/')
        .next_back()
        .filter(|s| !s.is_empty())
        .unwrap_or("download")
        .to_string()
}

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
    Fetching,
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
        let raw_filename = derive_initial_filename(&url);
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

#[cfg(test)]
mod tests {
    use super::*;

    // ─── sanitize_filename ───────────────────────────────────────────────

    #[test]
    fn sanitize_strips_path_traversal_components() {
        assert_eq!(sanitize_filename("../../etc/passwd"), "passwd");
        assert_eq!(sanitize_filename("..\\..\\windows\\cmd.exe"), "cmd.exe");
        assert_eq!(sanitize_filename("foo/bar/baz.mkv"), "baz.mkv");
        assert_eq!(sanitize_filename("C:\\Users\\foo.txt"), "foo.txt");
    }

    #[test]
    fn sanitize_removes_control_and_bidi_chars() {
        assert_eq!(sanitize_filename("ev\u{0000}il.exe"), "evil.exe");
        assert_eq!(sanitize_filename("tab\there.txt"), "tabhere.txt");
        assert_eq!(sanitize_filename("bell\x07.txt"), "bell.txt");
        // U+202E right-to-left override
        assert_eq!(sanitize_filename("exe\u{202E}txt.cod"), "exetxt.cod");
        // U+2066 left-to-right isolate
        assert_eq!(sanitize_filename("a\u{2066}b.txt"), "ab.txt");
    }

    #[test]
    fn sanitize_replaces_windows_illegal_chars() {
        assert_eq!(
            sanitize_filename(r#"foo<bar>:"|?*.txt"#),
            "foo_bar______.txt"
        );
    }

    #[test]
    fn sanitize_escapes_windows_reserved_names() {
        assert_eq!(sanitize_filename("CON"), "_CON");
        assert_eq!(sanitize_filename("nul.txt"), "_nul.txt");
        assert_eq!(sanitize_filename("COM1"), "_COM1");
        assert_eq!(sanitize_filename("LPT9.log"), "_LPT9.log");
        // lower case + case-insensitive match
        assert_eq!(sanitize_filename("aux"), "_aux");
        // Not reserved: CON_X or similar
        assert_eq!(sanitize_filename("CON_X.txt"), "CON_X.txt");
    }

    #[test]
    fn sanitize_trims_trailing_dots_and_spaces() {
        assert_eq!(sanitize_filename("foo.txt..."), "foo.txt");
        assert_eq!(sanitize_filename("bar   "), "bar");
        assert_eq!(sanitize_filename("baz. . ."), "baz");
    }

    #[test]
    fn sanitize_preserves_single_leading_dot() {
        assert_eq!(sanitize_filename(".gitignore"), ".gitignore");
        assert_eq!(sanitize_filename(".env"), ".env");
    }

    #[test]
    fn sanitize_strips_double_leading_dots() {
        // ".." is a path-traversal residue — shouldn't become a filename itself
        let out = sanitize_filename("..");
        assert!(out.starts_with("download-"));
    }

    #[test]
    fn sanitize_empty_becomes_fallback_uuid() {
        let a = sanitize_filename("");
        let b = sanitize_filename(".");
        assert!(a.starts_with("download-") && a.len() > "download-".len());
        assert!(b.starts_with("download-"));
    }

    #[test]
    fn sanitize_decodes_percent_encoding_then_sanitizes() {
        // %2E = '.' — %2E%2E%2Fpasswd decodes to "../passwd" which then strips
        assert_eq!(sanitize_filename("%2E%2E%2Fpasswd"), "passwd");
    }

    #[test]
    fn sanitize_keeps_unicode_body() {
        // Real-world: non-ASCII filenames are fine
        assert_eq!(sanitize_filename("résumé.pdf"), "résumé.pdf");
        assert_eq!(sanitize_filename("文件.zip"), "文件.zip");
    }

    // ─── derive_initial_filename ─────────────────────────────────────────

    #[test]
    fn derive_http_url_uses_last_segment() {
        assert_eq!(
            derive_initial_filename("https://example.com/path/to/foo.iso"),
            "foo.iso"
        );
    }

    #[test]
    fn derive_http_trailing_slash_falls_back() {
        // Trailing slash → empty last segment → fall back to "download"
        assert_eq!(derive_initial_filename("https://example.com/"), "download");
    }

    #[test]
    fn derive_magnet_uses_dn_param_not_tracker_path() {
        // Regression test for bug where magnets with trackers ending in
        // "/announce" produced a filename literally called "announce".
        let magnet = "magnet:?xt=urn:btih:07A9DE9750158471C3302E4E95EDB1107F980FA6\
                     &dn=Pioneer+One+S01E01+720p+x264+VODO\
                     &tr=http%3a%2f%2ftracker.opentrackr.org%3a1337%2fannounce";
        assert_eq!(
            derive_initial_filename(magnet),
            "Pioneer One S01E01 720p x264 VODO"
        );
    }

    #[test]
    fn derive_magnet_with_percent_encoding_in_dn() {
        let magnet =
            "magnet:?xt=urn:btih:0000000000000000000000000000000000000000&dn=foo%20bar.mkv";
        assert_eq!(derive_initial_filename(magnet), "foo bar.mkv");
    }

    #[test]
    fn derive_magnet_without_dn_falls_back_to_stub() {
        let magnet = "magnet:?xt=urn:btih:0000000000000000000000000000000000000000";
        assert_eq!(derive_initial_filename(magnet), "torrent-download");
    }

    #[test]
    fn derive_magnet_empty_dn_falls_back() {
        let magnet = "magnet:?xt=urn:btih:0000000000000000000000000000000000000000&dn=";
        assert_eq!(derive_initial_filename(magnet), "torrent-download");
    }
}
