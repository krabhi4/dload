use crate::domain::{jwt_secret, Claims, Download, DownloadStatus};
use crate::manager::SharedState;
use axum::{
    extract::{Path, Query, State},
    response::Json,
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
        return axum::response::IntoResponse::into_response((
            axum::http::StatusCode::FORBIDDEN,
            e,
        ));
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
    Json(payload): Json<serde_json::Value>,
) -> axum::response::Response {
    if require_auth(extract_token(&headers)).is_err() {
        return axum::response::IntoResponse::into_response((
            axum::http::StatusCode::UNAUTHORIZED,
            "Authentication required",
        ));
    }
    let url = payload["url"].as_str().unwrap_or("").trim().to_string();

    if let Err(e) = validate_download_url(&url) {
        return axum::response::IntoResponse::into_response((
            axum::http::StatusCode::BAD_REQUEST,
            e,
        ));
    }

    let settings = state.settings.read().await;
    let download_dir = settings.download_dir.clone();
    drop(settings);

    let download = Download::new(url, &download_dir);

    state.add_and_maybe_start(download.clone()).await;

    // Re-read to get the actual status (Downloading or Queued)
    let actual = {
        let downloads = state.downloads.read().await;
        downloads.get(&download.id).cloned().unwrap_or(download)
    };
    axum::response::IntoResponse::into_response(Json(actual))
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

    // Block private/internal IPs (SSRF protection)
    if let Some(host) = parsed.host_str() {
        // Block obvious internal hostnames
        let host_lower = host.to_lowercase();
        if host_lower == "localhost"
            || host_lower == "metadata.google.internal"
            || host_lower.ends_with(".internal")
            || host_lower.ends_with(".local")
        {
            return Err("Internal hosts not allowed".to_string());
        }

        // Block private IP ranges
        if let Ok(ip) = host.parse::<IpAddr>() {
            if is_private_ip(&ip) {
                return Err("Private/internal IP addresses not allowed".to_string());
            }
        }

        // Block URLs with embedded credentials
        if parsed.username() != "" || parsed.password().is_some() {
            return Err("URLs with credentials not allowed".to_string());
        }
    } else {
        return Err("URL must have a host".to_string());
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

    if let Some(host) = parsed.host_str() {
        let host_lower = host.to_lowercase();
        if host_lower == "localhost"
            || host_lower == "metadata.google.internal"
            || host_lower.ends_with(".internal")
            || host_lower.ends_with(".local")
        {
            return Err("Internal hosts not allowed".to_string());
        }

        if let Ok(ip) = host.parse::<IpAddr>() {
            if is_private_ip(&ip) {
                return Err("Private/internal IP addresses not allowed".to_string());
            }
        }

        if parsed.username() != "" || parsed.password().is_some() {
            return Err("URLs with embedded credentials not allowed".to_string());
        }
    } else {
        return Err("Mirror URL must have a valid host".to_string());
    }

    Ok(())
}
