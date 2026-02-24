use crate::domain::Download;
use crate::manager::SharedState;
use axum::{
    extract::{Path, State},
    response::Json,
    routing::{delete, get, post},
    Router,
};
use std::sync::Arc;

pub fn router(state: SharedState) -> Router {
    Router::new()
        .route("/api/downloads", get(list_downloads).post(add_download))
        .route("/api/downloads/:id", delete(remove_download))
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
) -> Json<serde_json::Value> {
    state.remove(&id).await;
    Json(serde_json::json!({ "success": true }))
}
