pub mod http;

use crate::domain::Protocol;

pub fn detect_protocol(url: &str) -> Protocol {
    let url_lower = url.to_lowercase();
    if url_lower.starts_with("magnet:") || url_lower.ends_with(".torrent") {
        Protocol::Torrent
    } else if url_lower.starts_with("sftp://") || url_lower.starts_with("ssh://") {
        Protocol::Sftp
    } else if url_lower.starts_with("ftp://") {
        Protocol::Ftp
    } else {
        Protocol::Http
    }
}
