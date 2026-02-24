use crate::domain::Download;
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

pub fn router(state: SharedState) -> Router {
    Router::new()
        .route("/api/downloads", get(list_downloads).post(add_download))
        .route("/api/downloads/:id", delete(remove_download))
        .route("/api/downloads/:id/pause", post(pause_download))
        .route("/api/downloads/:id/cancel", post(cancel_download))
        .route("/api/downloads/:id/resume", post(resume_download))
        .with_state(state)
}

async fn list_downloads(State(state): State<SharedState>) -> Json<Vec<Download>> {
    Json(state.get_all().await)
}

async fn add_download(
    State(state): State<SharedState>,
    Json(payload): Json<serde_json::Value>,
) -> Json<Download> {
    let url = payload["url"].as_str().unwrap_or("").to_string();
    let settings = state.settings.read().await;
    let download_dir = settings.download_dir.clone();
    drop(settings);

    let mut download = Download::new(url, &download_dir);
    download.status = crate::domain::DownloadStatus::Downloading;

    state.add_download(download.clone()).await;

    let state_clone = Arc::clone(&state);
    state_clone.start_download(download.clone()).await;

    Json(download)
}

async fn remove_download(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    Query(params): Query<DeleteParams>,
) -> Json<serde_json::Value> {
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
) -> Json<serde_json::Value> {
    state.pause_download(&id).await;
    Json(serde_json::json!({ "success": true }))
}

async fn cancel_download(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> Json<serde_json::Value> {
    state.cancel_download(&id).await;
    // Mark as stopped and persist
    let download = {
        let mut downloads = state.downloads.write().await;
        if let Some(d) = downloads.get_mut(&id) {
            d.status = crate::domain::DownloadStatus::Stopped;
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
) -> Json<serde_json::Value> {
    state.resume_download(&id).await;
    Json(serde_json::json!({ "success": true }))
}
