pub mod http;
pub mod torrent;

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

/// Extract the info_hash from a magnet URI.
/// Returns the 40-char lowercase hex hash, or None if not found.
pub fn extract_info_hash(url: &str) -> Option<String> {
    let url_lower = url.to_lowercase();
    if !url_lower.starts_with("magnet:") {
        return None;
    }

    // Look for xt=urn:btih:<hash> (case-insensitive parameter matching)
    for part in url_lower.split('&') {
        let part = part.strip_prefix("magnet:?").unwrap_or(part);
        if let Some(hash) = part.strip_prefix("xt=urn:btih:") {
            if hash.len() >= 40 {
                return Some(hash[..40].to_string());
            } else if hash.len() == 32 {
                return base32_to_hex(hash);
            }
            return None;
        }
    }
    None
}

fn base32_to_hex(input: &str) -> Option<String> {
    let alphabet = b"abcdefghijklmnopqrstuvwxyz234567";
    let mut bits: Vec<u8> = Vec::new();
    for c in input.bytes() {
        let val = alphabet.iter().position(|&b| b == c)?;
        for i in (0..5).rev() {
            bits.push((val >> i) as u8 & 1);
        }
    }
    let mut hex = String::new();
    for chunk in bits.chunks(4) {
        if chunk.len() == 4 {
            let val = chunk[0] * 8 + chunk[1] * 4 + chunk[2] * 2 + chunk[3];
            hex.push(char::from_digit(val as u32, 16)?);
        }
    }
    if hex.len() == 40 {
        Some(hex)
    } else {
        None
    }
}
