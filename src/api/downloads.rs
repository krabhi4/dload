use crate::domain::{jwt_secret, Claims, Download, DownloadStatus};
use crate::manager::SharedState;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::{delete, get, post},
    Router,
};
use std::net::IpAddr;
use std::sync::Arc;

#[derive(serde::Deserialize)]
pub struct DeleteParams {
    #[serde(default)]
    delete_files: bool,
}

#[derive(serde::Deserialize)]
pub struct HttpMirrorRequest {
    url: String,
    #[serde(default = "default_true")]
    keep_seeding: bool,
}

fn default_true() -> bool {
    true
}

fn extract_token(headers: &axum::http::HeaderMap) -> &str {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("")
}

fn require_auth(token: &str) -> Result<Claims, String> {
    jsonwebtoken::decode::<Claims>(
        token,
        &jsonwebtoken::DecodingKey::from_secret(jwt_secret()),
        &jsonwebtoken::Validation::default(),
    )
    .map(|d| d.claims)
    .map_err(|_| "Authentication required".to_string())
}

fn require_admin(token: &str) -> Result<Claims, String> {
    jsonwebtoken::decode::<Claims>(
        token,
        &jsonwebtoken::DecodingKey::from_secret(jwt_secret()),
        &jsonwebtoken::Validation::default(),
    )
    .map(|d| d.claims)
    .map_err(|_| "Invalid token".to_string())
    .and_then(|c| {
        if c.role == "ADMIN" {
            Ok(c)
        } else {
            Err("Admin access required".to_string())
        }
    })
}

#[derive(serde::Deserialize)]
struct ReorderRequest {
    ids: Vec<String>,
}

pub fn router(state: SharedState) -> Router {
    Router::new()
        .route("/api/downloads", get(list_downloads).post(add_download))
        .route("/api/downloads/reorder", post(reorder_downloads_handler))
        .route("/api/downloads/:id", delete(remove_download))
        .route("/api/downloads/:id/pause", post(pause_download))
        .route("/api/downloads/:id/cancel", post(cancel_download))
        .route("/api/downloads/:id/resume", post(resume_download))
        .route("/api/downloads/:id/torrent", get(export_torrent))
        .route("/api/downloads/:id/http-mirror", post(start_http_mirror))
        .route("/api/downloads/resume-all", post(resume_all_downloads))
        .with_state(state)
}

async fn list_downloads(
    State(state): State<SharedState>,
    headers: axum::http::HeaderMap,
) -> axum::response::Response {
    if require_auth(extract_token(&headers)).is_err() {
        return axum::response::IntoResponse::into_response((
            axum::http::StatusCode::UNAUTHORIZED,
            "Authentication required",
        ));
    }
    axum::response::IntoResponse::into_response(Json(state.get_all().await))
}

async fn reorder_downloads_handler(
    State(state): State<SharedState>,
    headers: axum::http::HeaderMap,
    Json(payload): Json<ReorderRequest>,
) -> axum::response::Response {
    if let Err(e) = require_admin(extract_token(&headers)) {
        return axum::response::IntoResponse::into_response((axum::http::StatusCode::FORBIDDEN, e));
    }

    if payload.ids.len() > 1000 {
        return axum::response::IntoResponse::into_response((
            axum::http::StatusCode::BAD_REQUEST,
            "Too many IDs",
        ));
    }

    state.reorder_downloads(payload.ids).await;
    axum::response::IntoResponse::into_response(Json(serde_json::json!({ "success": true })))
}

async fn add_download(
    State(state): State<SharedState>,
    headers: axum::http::HeaderMap,
    request: axum::extract::Request,
) -> axum::response::Response {
    if require_auth(extract_token(&headers)).is_err() {
        return (StatusCode::UNAUTHORIZED, "Authentication required").into_response();
    }

    let content_type = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/json");

    if content_type.starts_with("multipart/form-data") {
        use axum::extract::FromRequest;
        match axum_extra::extract::Multipart::from_request(request, &state).await {
            Ok(mp) => handle_add_download_multipart(state, mp).await,
            Err(e) => (
                StatusCode::BAD_REQUEST,
                format!("Invalid multipart body: {}", e),
            )
                .into_response(),
        }
    } else if content_type.starts_with("application/json") {
        const MAX_JSON_BYTES: usize = 1024 * 1024; // 1 MiB
        let body_bytes = match axum::body::to_bytes(request.into_body(), MAX_JSON_BYTES).await {
            Ok(b) => b,
            Err(_) => {
                return (StatusCode::PAYLOAD_TOO_LARGE, "Request body too large").into_response()
            }
        };
        let payload: serde_json::Value = match serde_json::from_slice(&body_bytes) {
            Ok(v) => v,
            Err(e) => {
                return (StatusCode::BAD_REQUEST, format!("Invalid JSON: {}", e)).into_response()
            }
        };
        let url = payload["url"].as_str().unwrap_or("").trim().to_string();
        let folder_id = payload["folder_id"].as_str();

        if let Err(e) = validate_download_url(&url) {
            return (StatusCode::BAD_REQUEST, e).into_response();
        }

        let download_dir = {
            let settings = state.settings.read().await;
            match folder_id {
                Some(id) => settings
                    .folder_path_by_id(id)
                    .unwrap_or(settings.default_folder_path())
                    .to_string(),
                None => settings.default_folder_path().to_string(),
            }
        };

        let download = Download::new(url, &download_dir);

        state.add_and_maybe_start(download.clone()).await;

        // Re-read to get the actual status (Downloading or Queued).
        let actual = {
            let downloads = state.downloads.read().await;
            downloads.get(&download.id).cloned().unwrap_or(download)
        };
        Json(actual).into_response()
    } else {
        (
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            format!("Unsupported content type: {}", content_type),
        )
            .into_response()
    }
}

async fn handle_add_download_multipart(
    state: SharedState,
    mut multipart: axum_extra::extract::Multipart,
) -> axum::response::Response {
    const MAX_TORRENT_BYTES: usize = 10 * 1024 * 1024; // 10 MiB per file
    const MAX_TOTAL_TORRENT_BYTES: usize = 100 * 1024 * 1024; // 100 MiB aggregate

    let mut torrent_blobs: Vec<Vec<u8>> = Vec::new();
    let mut urls: Vec<String> = Vec::new();
    let mut folder_id: Option<String> = None;
    let mut total: usize = 0;

    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "torrents" => {
                let bytes = match field.bytes().await {
                    Ok(b) => b,
                    Err(e) => {
                        return (
                            StatusCode::BAD_REQUEST,
                            format!("Failed to read torrent field: {}", e),
                        )
                            .into_response()
                    }
                };
                if bytes.len() > MAX_TORRENT_BYTES {
                    return (
                        StatusCode::PAYLOAD_TOO_LARGE,
                        "Torrent file too large (max 10 MiB)",
                    )
                        .into_response();
                }
                total = total.saturating_add(bytes.len());
                if total > MAX_TOTAL_TORRENT_BYTES {
                    return (
                        StatusCode::PAYLOAD_TOO_LARGE,
                        "Aggregate torrent payload too large (max 100 MiB)",
                    )
                        .into_response();
                }
                if !bytes.is_empty() {
                    torrent_blobs.push(bytes.to_vec());
                }
            }
            "urls" => {
                let text = match field.text().await {
                    Ok(t) => t,
                    Err(e) => {
                        return (
                            StatusCode::BAD_REQUEST,
                            format!("Failed to read urls field: {}", e),
                        )
                            .into_response()
                    }
                };
                for line in text.lines() {
                    let line = line.trim();
                    if !line.is_empty() {
                        urls.push(line.to_string());
                    }
                }
            }
            "folder_id" => {
                if let Ok(text) = field.text().await {
                    let text = text.trim().to_string();
                    if !text.is_empty() {
                        folder_id = Some(text);
                    }
                }
            }
            _ => {
                // Drain unknown fields so the multipart reader advances.
                if let Err(e) = field.bytes().await {
                    tracing::warn!("failed to consume multipart field '{}': {}", name, e);
                }
            }
        }
    }

    if torrent_blobs.is_empty() && urls.is_empty() {
        return (StatusCode::BAD_REQUEST, "No torrent files or urls provided").into_response();
    }

    // Validate every url up-front — reject the whole submission if any is bad,
    // so the client gets a single clear error rather than partial success.
    for url in &urls {
        if let Err(e) = validate_download_url(url) {
            return (
                StatusCode::BAD_REQUEST,
                format!("Invalid url '{}': {}", url, e),
            )
                .into_response();
        }
    }

    let download_dir = {
        let settings = state.settings.read().await;
        match folder_id.as_deref() {
            Some(id) => settings
                .folder_path_by_id(id)
                .unwrap_or(settings.default_folder_path())
                .to_string(),
            None => settings.default_folder_path().to_string(),
        }
    };

    let mut created: Vec<Download> = Vec::new();

    for url in urls {
        let download = Download::new(url, &download_dir);
        state.add_and_maybe_start(download.clone()).await;
        let actual = {
            let downloads = state.downloads.read().await;
            downloads.get(&download.id).cloned().unwrap_or(download)
        };
        created.push(actual);
    }

    let mut skipped_torrents = 0usize;
    for bytes in torrent_blobs {
        // Category is not exposed on the native API; the qBit-compat layer
        // carries it through a separate endpoint. Pass None here.
        match state
            .add_torrent_from_bytes(bytes, &download_dir, None)
            .await
        {
            Some(dl) => created.push(dl),
            None => skipped_torrents += 1,
        }
    }

    if created.is_empty() && skipped_torrents > 0 {
        return (
            StatusCode::BAD_REQUEST,
            "All uploaded torrent files were invalid",
        )
            .into_response();
    }

    Json(created).into_response()
}

async fn remove_download(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    Query(params): Query<DeleteParams>,
    headers: axum::http::HeaderMap,
) -> Json<serde_json::Value> {
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");

    if let Err(e) = require_admin(token) {
        return Json(serde_json::json!({
            "success": false,
            "error": e
        }));
    }

    if params.delete_files {
        state.remove_with_files(&id).await;
    } else {
        state.remove(&id).await;
    }
    Json(serde_json::json!({ "success": true }))
}

async fn pause_download(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    headers: axum::http::HeaderMap,
) -> Json<serde_json::Value> {
    if let Err(e) = require_admin(extract_token(&headers)) {
        return Json(serde_json::json!({ "success": false, "error": e }));
    }
    state.pause_download(&id).await;
    Json(serde_json::json!({ "success": true }))
}

async fn cancel_download(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    headers: axum::http::HeaderMap,
) -> Json<serde_json::Value> {
    if let Err(e) = require_admin(extract_token(&headers)) {
        return Json(serde_json::json!({ "success": false, "error": e }));
    }
    state.cancel_download(&id).await;
    let download = {
        let mut downloads = state.downloads.write().await;
        if let Some(d) = downloads.get_mut(&id) {
            if d.status == DownloadStatus::Seeding {
                d.status = DownloadStatus::Completed;
                d.progress = 100.0;
            } else {
                d.status = DownloadStatus::Stopped;
            }
            d.speed = 0;
            d.upload_speed = 0;
            Some(d.clone())
        } else {
            None
        }
    };
    if let Some(d) = download {
        state.update_download(&d).await;
    }
    Json(serde_json::json!({ "success": true }))
}

async fn resume_download(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    headers: axum::http::HeaderMap,
) -> Json<serde_json::Value> {
    if let Err(e) = require_admin(extract_token(&headers)) {
        return Json(serde_json::json!({ "success": false, "error": e }));
    }
    state.resume_download(&id).await;
    Json(serde_json::json!({ "success": true }))
}

async fn resume_all_downloads(
    State(state): State<SharedState>,
    headers: axum::http::HeaderMap,
) -> Json<serde_json::Value> {
    if let Err(e) = require_admin(extract_token(&headers)) {
        return Json(serde_json::json!({ "success": false, "error": e }));
    }

    let ids: Vec<String> = state
        .get_all()
        .await
        .iter()
        .filter(|d| {
            d.status == DownloadStatus::Paused
                || d.status == DownloadStatus::Failed
                || d.status == DownloadStatus::Stopped
                || d.status == DownloadStatus::Queued
        })
        .map(|d| d.id.clone())
        .collect();

    let count = ids.len();

    // Spawn throttled resume in background so API responds immediately
    let state_clone = Arc::clone(&state);
    tokio::spawn(async move {
        state_clone.resume_all_downloads(ids, 2).await;
    });

    Json(serde_json::json!({ "success": true, "count": count }))
}

async fn export_torrent(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    headers: axum::http::HeaderMap,
) -> axum::response::Response {
    if require_auth(extract_token(&headers)).is_err() {
        return axum::response::IntoResponse::into_response((
            axum::http::StatusCode::UNAUTHORIZED,
            "Authentication required",
        ));
    }

    let download = state.get_download(&id).await;

    let download = match download {
        Some(d) => d,
        None => {
            return axum::response::IntoResponse::into_response((
                axum::http::StatusCode::NOT_FOUND,
                "Download not found",
            ))
        }
    };

    if download.protocol != crate::domain::Protocol::Torrent {
        return axum::response::IntoResponse::into_response((
            axum::http::StatusCode::BAD_REQUEST,
            "Not a torrent download",
        ));
    }

    let bytes = match state.export_torrent_bytes(&id).await {
        Some(b) => b,
        None => {
            return axum::response::IntoResponse::into_response((
                axum::http::StatusCode::NOT_FOUND,
                "Torrent session not active — cannot export .torrent file",
            ))
        }
    };

    // Sanitize filename for Content-Disposition
    let safe_name = crate::domain::sanitize_filename(&download.filename);
    let disposition = format!("attachment; filename=\"{}.torrent\"", safe_name);

    axum::response::Response::builder()
        .status(axum::http::StatusCode::OK)
        .header("Content-Type", "application/x-bittorrent")
        .header("Content-Disposition", disposition)
        .body(axum::body::Body::from(bytes))
        .unwrap()
}

async fn start_http_mirror(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    headers: axum::http::HeaderMap,
    Json(payload): Json<HttpMirrorRequest>,
) -> axum::response::Response {
    if let Err(e) = require_admin(extract_token(&headers)) {
        return axum::response::IntoResponse::into_response((
            axum::http::StatusCode::UNAUTHORIZED,
            e,
        ));
    }

    let url = payload.url.trim().to_string();
    if url.is_empty() {
        return axum::response::IntoResponse::into_response((
            axum::http::StatusCode::BAD_REQUEST,
            "URL is required",
        ));
    }

    // Reuse the same validation as regular downloads (SSRF, scheme, credentials, etc.)
    if let Err(e) = validate_mirror_url(&url) {
        return axum::response::IntoResponse::into_response((
            axum::http::StatusCode::BAD_REQUEST,
            e,
        ));
    }

    let download = state.get_download(&id).await;
    let download = match download {
        Some(d) => d,
        None => {
            return axum::response::IntoResponse::into_response((
                axum::http::StatusCode::NOT_FOUND,
                "Download not found",
            ));
        }
    };

    if download.protocol != crate::domain::Protocol::Torrent {
        return axum::response::IntoResponse::into_response((
            axum::http::StatusCode::BAD_REQUEST,
            "HTTP mirror is only available for torrent downloads",
        ));
    }

    if download.http_mirror_status.is_some() {
        return axum::response::IntoResponse::into_response((
            axum::http::StatusCode::CONFLICT,
            "HTTP mirror is already in progress for this download",
        ));
    }

    match download.status {
        DownloadStatus::Downloading | DownloadStatus::Paused | DownloadStatus::Seeding => {}
        _ => {
            return axum::response::IntoResponse::into_response((
                axum::http::StatusCode::BAD_REQUEST,
                "Download must be in Downloading, Paused, or Seeding status",
            ));
        }
    }

    let state_clone = Arc::clone(&state);
    state_clone
        .start_http_mirror(id, url, payload.keep_seeding)
        .await;

    axum::response::IntoResponse::into_response((
        axum::http::StatusCode::OK,
        Json(serde_json::json!({ "success": true })),
    ))
}

fn validate_download_url(url: &str) -> Result<(), String> {
    if url.is_empty() {
        return Err("URL is required".to_string());
    }

    if url.len() > 4096 {
        return Err("URL too long".to_string());
    }

    // Allow magnet links directly
    if url.starts_with("magnet:") {
        return Ok(());
    }

    let parsed = url::Url::parse(url).map_err(|_| "Invalid URL format".to_string())?;

    // Whitelist allowed schemes
    match parsed.scheme() {
        "http" | "https" | "ftp" => {}
        _ => return Err(format!("Unsupported protocol: {}", parsed.scheme())),
    }

    // Block URLs with embedded credentials
    if parsed.username() != "" || parsed.password().is_some() {
        return Err("URLs with credentials not allowed".to_string());
    }

    // Block private/internal hosts (SSRF protection). Use `Url::host()` (not
    // `host_str()`) so bracketed IPv6 literals like `[::1]` are parsed directly
    // rather than string-parsed (which fails due to the brackets and silently
    // skips the IP check).
    match parsed.host() {
        Some(url::Host::Domain(host)) => {
            let host_lower = host.to_ascii_lowercase();
            if host_lower == "localhost"
                || host_lower == "metadata.google.internal"
                || host_lower.ends_with(".internal")
                || host_lower.ends_with(".local")
            {
                return Err("Internal hosts not allowed".to_string());
            }
        }
        Some(url::Host::Ipv4(v4)) => {
            if is_private_ip(&IpAddr::V4(v4)) {
                return Err("Private/internal IP addresses not allowed".to_string());
            }
        }
        Some(url::Host::Ipv6(v6)) => {
            if is_private_ip(&IpAddr::V6(v6)) {
                return Err("Private/internal IP addresses not allowed".to_string());
            }
        }
        None => return Err("URL must have a host".to_string()),
    }

    Ok(())
}

fn is_private_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()                           // 127.0.0.0/8
                || v4.is_private()                     // 10/8, 172.16/12, 192.168/16
                || v4.is_link_local()                  // 169.254.0.0/16 (AWS metadata etc.)
                || v4.is_broadcast()                   // 255.255.255.255
                || v4.is_unspecified()                 // 0.0.0.0
                || v4.octets()[0] == 100 && (v4.octets()[1] & 0xC0) == 64 // 100.64.0.0/10 (CGNAT)
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

/// Validate mirror URL: must be HTTP/HTTPS, not private/internal.
fn validate_mirror_url(url: &str) -> Result<(), String> {
    let parsed = url::Url::parse(url).map_err(|_| "Invalid URL format".to_string())?;

    match parsed.scheme() {
        "http" | "https" => {}
        _ => return Err("Only HTTP/HTTPS URLs are supported for mirrors".to_string()),
    }

    if parsed.username() != "" || parsed.password().is_some() {
        return Err("URLs with embedded credentials not allowed".to_string());
    }

    match parsed.host() {
        Some(url::Host::Domain(host)) => {
            let host_lower = host.to_ascii_lowercase();
            if host_lower == "localhost"
                || host_lower == "metadata.google.internal"
                || host_lower.ends_with(".internal")
                || host_lower.ends_with(".local")
            {
                return Err("Internal hosts not allowed".to_string());
            }
        }
        Some(url::Host::Ipv4(v4)) => {
            if is_private_ip(&IpAddr::V4(v4)) {
                return Err("Private/internal IP addresses not allowed".to_string());
            }
        }
        Some(url::Host::Ipv6(v6)) => {
            if is_private_ip(&IpAddr::V6(v6)) {
                return Err("Private/internal IP addresses not allowed".to_string());
            }
        }
        None => return Err("Mirror URL must have a valid host".to_string()),
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(u: &str) {
        assert!(
            validate_download_url(u).is_ok(),
            "expected OK, got {:?} for {u}",
            validate_download_url(u)
        );
    }
    fn err(u: &str, contains: &str) {
        match validate_download_url(u) {
            Err(e) => assert!(
                e.contains(contains),
                "err for {u} was {e:?}, expected contains {contains:?}"
            ),
            Ok(()) => panic!("expected Err containing {contains:?} for {u}, got Ok"),
        }
    }

    // ─── validate_download_url ───────────────────────────────────────────

    #[test]
    fn accepts_public_http_https_ftp() {
        ok("https://example.com/file.iso");
        ok("http://example.com/file.iso");
        ok("ftp://ftp.example.com/pub/file.iso");
    }

    #[test]
    fn accepts_magnet_without_further_validation() {
        ok("magnet:?xt=urn:btih:0000000000000000000000000000000000000000");
    }

    #[test]
    fn rejects_unsupported_schemes() {
        err("file:///etc/passwd", "Unsupported protocol");
        err("gopher://example.com/", "Unsupported protocol");
        err("javascript:alert(1)", "Unsupported protocol");
    }

    #[test]
    fn rejects_ipv4_private_ranges() {
        err("http://127.0.0.1/x", "Private/internal");
        err("http://10.0.0.5/x", "Private/internal");
        err("http://172.16.0.1/x", "Private/internal");
        err("http://192.168.1.1/x", "Private/internal");
        err("http://169.254.169.254/x", "Private/internal"); // AWS metadata
        err("http://100.64.0.1/x", "Private/internal"); // CGNAT
        err("http://0.0.0.0/x", "Private/internal");
    }

    #[test]
    fn rejects_ipv6_private_and_mapped() {
        err("http://[::1]/x", "Private/internal");
        err("http://[::]/x", "Private/internal");
        err("http://[::ffff:127.0.0.1]/x", "Private/internal");
    }

    #[test]
    fn rejects_internal_hostnames() {
        err("http://localhost/x", "Internal");
        err("http://LocalHost/x", "Internal");
        err("http://metadata.google.internal/x", "Internal");
        err("http://db.internal/x", "Internal");
        err("http://printer.local/x", "Internal");
    }

    #[test]
    fn rejects_credentialed_urls() {
        err("https://user:pass@example.com/x", "credentials");
        err("https://user@example.com/x", "credentials");
    }

    #[test]
    fn rejects_overlength_and_empty() {
        err("", ""); // any error
        let long = format!("https://example.com/{}", "a".repeat(5000));
        err(&long, "too long");
    }

    // ─── is_private_ip ───────────────────────────────────────────────────

    #[test]
    fn is_private_ip_matrix() {
        let private = [
            "127.0.0.1",
            "127.255.255.255",
            "10.0.0.1",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.0.1",
            "169.254.1.1",
            "100.64.0.1",
            "100.127.255.255",
            "0.0.0.0",
            "::1",
            "::",
        ];
        let public = [
            "1.1.1.1",
            "8.8.8.8",
            "9.9.9.9",
            "172.32.0.1",           // outside 172.16/12
            "100.128.0.1",          // outside 100.64/10
            "2606:4700:4700::1111", // Cloudflare DNS
        ];
        for ip in &private {
            let parsed: std::net::IpAddr = ip.parse().unwrap();
            assert!(is_private_ip(&parsed), "{ip} should be private");
        }
        for ip in &public {
            let parsed: std::net::IpAddr = ip.parse().unwrap();
            assert!(!is_private_ip(&parsed), "{ip} should be public");
        }
    }
}
