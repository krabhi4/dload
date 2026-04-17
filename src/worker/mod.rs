pub mod http;
pub mod mirror;
pub mod torrent;

use crate::domain::Protocol;
use std::net::IpAddr;

/// Redirect policy that refuses to follow `Location` headers pointing at private
/// or loopback addresses, internal hostnames, or after too many hops. Mirrors
/// the SSRF checks in api/downloads.rs so servers can't redirect us there.
pub fn ssrf_safe_redirect_policy() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        if attempt.previous().len() >= 5 {
            return attempt.error("too many redirects");
        }
        let url = attempt.url();
        // Credentials embedded in a redirect target are a classic way to launder
        // a leaked token or trick servers into auth-forwarding.
        if !url.username().is_empty() || url.password().is_some() {
            return attempt.error("redirect with embedded credentials blocked");
        }
        if let Some(host) = url.host_str() {
            let host_lower = host.to_ascii_lowercase();
            if host_lower == "localhost"
                || host_lower == "metadata.google.internal"
                || host_lower.ends_with(".internal")
                || host_lower.ends_with(".local")
            {
                return attempt.error("redirect to internal host blocked");
            }
            if let Ok(ip) = host.parse::<IpAddr>() {
                if is_private_ip(&ip) {
                    return attempt.error("redirect to private IP blocked");
                }
            }
        }
        attempt.follow()
    })
}

fn is_private_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_unspecified()
                || (v4.octets()[0] == 100 && (v4.octets()[1] & 0xC0) == 64)
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || v6
                    .to_ipv4_mapped()
                    .is_some_and(|v4| is_private_ip(&IpAddr::V4(v4)))
        }
    }
}

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
