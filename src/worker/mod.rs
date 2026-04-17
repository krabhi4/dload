pub mod http;
pub mod mirror;
pub mod torrent;

use crate::domain::Protocol;
use std::net::IpAddr;

/// Outcome of checking a redirect target against SSRF rules.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RedirectDecision {
    Follow,
    Block(&'static str),
}

/// Pure predicate: decide whether a redirect target is safe to follow. Kept
/// separate from the reqwest policy closure so it's unit-testable (reqwest's
/// `Attempt` type has no constructor we can synthesize in tests).
pub(crate) fn redirect_decision(url: &url::Url, previous_count: usize) -> RedirectDecision {
    if previous_count >= 5 {
        return RedirectDecision::Block("too many redirects");
    }
    if !url.username().is_empty() || url.password().is_some() {
        return RedirectDecision::Block("redirect with embedded credentials blocked");
    }
    match url.host() {
        Some(url::Host::Domain(host)) => {
            let host_lower = host.to_ascii_lowercase();
            if host_lower == "localhost"
                || host_lower == "metadata.google.internal"
                || host_lower.ends_with(".internal")
                || host_lower.ends_with(".local")
            {
                return RedirectDecision::Block("redirect to internal host blocked");
            }
        }
        Some(url::Host::Ipv4(v4)) if is_private_ip(&IpAddr::V4(v4)) => {
            return RedirectDecision::Block("redirect to private IP blocked");
        }
        Some(url::Host::Ipv6(v6)) if is_private_ip(&IpAddr::V6(v6)) => {
            return RedirectDecision::Block("redirect to private IP blocked");
        }
        _ => {}
    }
    RedirectDecision::Follow
}

/// Redirect policy that refuses to follow `Location` headers pointing at private
/// or loopback addresses, internal hostnames, URLs with embedded credentials,
/// or after too many hops. Mirrors the SSRF checks in api/downloads.rs so
/// servers can't redirect us there.
pub fn ssrf_safe_redirect_policy() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        match redirect_decision(attempt.url(), attempt.previous().len()) {
            RedirectDecision::Follow => attempt.follow(),
            RedirectDecision::Block(reason) => attempt.error(reason),
        }
    })
}

pub(crate) fn is_private_ip(ip: &IpAddr) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;

    // ─── detect_protocol ─────────────────────────────────────────────────

    #[test]
    fn detect_protocol_cases() {
        assert_eq!(
            detect_protocol("magnet:?xt=urn:btih:abc"),
            Protocol::Torrent
        );
        assert_eq!(
            detect_protocol("https://a.com/foo.torrent"),
            Protocol::Torrent
        );
        assert_eq!(
            detect_protocol("HTTPS://A.COM/FOO.TORRENT"),
            Protocol::Torrent
        );
        assert_eq!(detect_protocol("sftp://host/path"), Protocol::Sftp);
        assert_eq!(detect_protocol("ssh://host/path"), Protocol::Sftp);
        assert_eq!(detect_protocol("ftp://host/path"), Protocol::Ftp);
        assert_eq!(detect_protocol("https://a.com/file.iso"), Protocol::Http);
        assert_eq!(detect_protocol(""), Protocol::Http);
    }

    // ─── extract_info_hash ───────────────────────────────────────────────

    #[test]
    fn extract_hex_info_hash_lowercased() {
        let magnet = "magnet:?xt=urn:btih:07A9DE9750158471C3302E4E95EDB1107F980FA6&dn=x";
        assert_eq!(
            extract_info_hash(magnet),
            Some("07a9de9750158471c3302e4e95edb1107f980fa6".to_string())
        );
    }

    #[test]
    fn extract_hex_info_hash_truncates_extra_trailing() {
        let magnet = "magnet:?xt=urn:btih:00000000000000000000000000000000000000001234";
        assert_eq!(
            extract_info_hash(magnet),
            Some("0000000000000000000000000000000000000000".to_string())
        );
    }

    #[test]
    fn extract_base32_info_hash_32_chars_converted_to_40_hex() {
        // 32 base32 chars → 160 bits → 40 hex chars
        let magnet = "magnet:?xt=urn:btih:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let h = extract_info_hash(magnet).expect("should decode");
        assert_eq!(h.len(), 40);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn extract_info_hash_returns_none_without_magnet_prefix() {
        assert_eq!(extract_info_hash("https://example.com/file.torrent"), None);
    }

    #[test]
    fn extract_info_hash_none_on_short_hash() {
        assert_eq!(extract_info_hash("magnet:?xt=urn:btih:deadbeef"), None);
    }

    // ─── base32_to_hex ───────────────────────────────────────────────────

    #[test]
    fn base32_to_hex_rejects_invalid_chars() {
        // '1' is not in the base32 alphabet
        assert_eq!(base32_to_hex("11111111111111111111111111111111"), None);
    }

    #[test]
    fn base32_to_hex_rejects_wrong_length() {
        assert_eq!(base32_to_hex("aaaa"), None);
        assert_eq!(base32_to_hex(""), None);
    }

    // ─── is_private_ip ───────────────────────────────────────────────────

    #[test]
    fn worker_is_private_ip_matrix() {
        use std::net::IpAddr;
        for ip in &[
            "127.0.0.1",
            "10.0.0.1",
            "192.168.5.5",
            "169.254.169.254",
            "::1",
        ] {
            assert!(is_private_ip(&ip.parse::<IpAddr>().unwrap()));
        }
        for ip in &["8.8.8.8", "1.1.1.1", "2606:4700:4700::1111"] {
            assert!(!is_private_ip(&ip.parse::<IpAddr>().unwrap()));
        }
    }

    // ─── redirect_decision ───────────────────────────────────────────────

    fn u(s: &str) -> url::Url {
        url::Url::parse(s).unwrap()
    }

    #[test]
    fn redirect_follow_on_public_host() {
        assert_eq!(
            redirect_decision(&u("https://example.com/"), 0),
            RedirectDecision::Follow
        );
    }

    #[test]
    fn redirect_blocks_too_many_hops() {
        assert!(matches!(
            redirect_decision(&u("https://example.com/"), 5),
            RedirectDecision::Block(_)
        ));
    }

    #[test]
    fn redirect_blocks_credentials() {
        assert!(matches!(
            redirect_decision(&u("https://user:pass@example.com/"), 0),
            RedirectDecision::Block(msg) if msg.contains("credentials")
        ));
    }

    #[test]
    fn redirect_blocks_localhost_hostname() {
        assert!(matches!(
            redirect_decision(&u("http://localhost/x"), 0),
            RedirectDecision::Block(_)
        ));
    }

    #[test]
    fn redirect_blocks_internal_tlds() {
        assert!(matches!(
            redirect_decision(&u("http://db.internal/x"), 0),
            RedirectDecision::Block(_)
        ));
        assert!(matches!(
            redirect_decision(&u("http://printer.local/x"), 0),
            RedirectDecision::Block(_)
        ));
    }

    #[test]
    fn redirect_blocks_private_ipv4() {
        assert!(matches!(
            redirect_decision(&u("http://10.0.0.1/"), 0),
            RedirectDecision::Block(_)
        ));
        assert!(matches!(
            redirect_decision(&u("http://169.254.169.254/"), 0),
            RedirectDecision::Block(_)
        ));
    }

    #[test]
    fn redirect_blocks_bracketed_ipv6_loopback() {
        // Regression: host_str() returned "[::1]" which failed IpAddr::parse.
        // Url::host() returns url::Host::Ipv6(::1) directly.
        assert!(matches!(
            redirect_decision(&u("http://[::1]/"), 0),
            RedirectDecision::Block(_)
        ));
    }
}
