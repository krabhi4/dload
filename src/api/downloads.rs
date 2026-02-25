use crate::domain::{Claims, Download, DownloadStatus, JWT_SECRET};
use crate::manager::SharedState;
use axum::{
    extract::{Path, Query, State},
    response::Json,
    routing::{delete, get, post},
    Router,
};
use std::sync::Arc;

#[derive(serde::Deserialize)]
pub struct DeleteParams {
    #[serde(default)]
    delete_files: bool,
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
        &jsonwebtoken::DecodingKey::from_secret(JWT_SECRET),
        &jsonwebtoken::Validation::default(),
    )
    .map(|d| d.claims)
    .map_err(|_| "Authentication required".to_string())
}

fn require_admin(token: &str) -> Result<Claims, String> {
    jsonwebtoken::decode::<Claims>(
        token,
        &jsonwebtoken::DecodingKey::from_secret(JWT_SECRET),
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

pub fn router(state: SharedState) -> Router {
    Router::new()
        .route("/api/downloads", get(list_downloads).post(add_download))
        .route("/api/downloads/:id", delete(remove_download))
        .route("/api/downloads/:id/pause", post(pause_download))
        .route("/api/downloads/:id/cancel", post(cancel_download))
        .route("/api/downloads/:id/resume", post(resume_download))
        .with_state(state)
}

async fn list_downloads(
    State(state): State<SharedState>,
    headers: axum::http::HeaderMap,
) -> axum::response::Response {
    if require_auth(extract_token(&headers)).is_err() {
        return axum::response::IntoResponse::into_response(
            (axum::http::StatusCode::UNAUTHORIZED, "Authentication required")
        );
    }
    axum::response::IntoResponse::into_response(Json(state.get_all().await))
}

async fn add_download(
    State(state): State<SharedState>,
    headers: axum::http::HeaderMap,
    Json(payload): Json<serde_json::Value>,
) -> axum::response::Response {
    if require_auth(extract_token(&headers)).is_err() {
        return axum::response::IntoResponse::into_response(
            (axum::http::StatusCode::UNAUTHORIZED, "Authentication required")
        );
    }
    let url = payload["url"].as_str().unwrap_or("").to_string();
    let settings = state.settings.read().await;
    let download_dir = settings.download_dir.clone();
    drop(settings);

    let mut download = Download::new(url, &download_dir);
    download.status = crate::domain::DownloadStatus::Downloading;

    state.add_download(download.clone()).await;

    let state_clone = Arc::clone(&state);
    state_clone.start_download(download.clone()).await;

    axum::response::IntoResponse::into_response(Json(download))
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
