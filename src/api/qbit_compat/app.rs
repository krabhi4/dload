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
        "max_ratio_enabled": false,
        "max_ratio": -1,
        "max_ratio_act": 0,
        "max_seeding_time_enabled": false,
        "max_seeding_time": -1,
    }))
}

pub async fn transfer_info() -> impl IntoResponse {
    Json(serde_json::json!({
        "dl_info_speed": 0,
        "dl_info_data": 0,
        "up_info_speed": 0,
        "up_info_data": 0,
        "dl_rate_limit": 0,
        "up_rate_limit": 0,
        "dht_nodes": 0,
        "connection_status": "connected",
    }))
}
