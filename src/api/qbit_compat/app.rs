use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    Json,
};

use super::QbitState;

pub async fn version() -> impl IntoResponse {
    (StatusCode::OK, "v5.0.0")
}

pub async fn webapi_version() -> impl IntoResponse {
    (StatusCode::OK, "2.11.0")
}

pub async fn preferences(State(state): State<QbitState>) -> impl IntoResponse {
    let settings = state.manager.settings.read().await;
    Json(serde_json::json!({
        "save_path": settings.download_dir,
        "max_active_downloads": settings.max_concurrent,
        "max_active_torrents": settings.max_concurrent,
        "max_active_uploads": settings.max_concurrent,
        "locale": "en",
        "dht": true,
        "pex": true,
        "lsd": true,
        "encryption": 0,
        "queueing_enabled": true,
    }))
}
