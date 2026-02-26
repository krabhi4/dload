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
    let save_path = format!("{}/", settings.download_dir.trim_end_matches('/'));
    Json(serde_json::json!({
        "save_path": save_path,
        "max_active_downloads": settings.max_concurrent,
        "max_active_torrents": settings.max_concurrent,
        "max_active_uploads": settings.max_concurrent,
        "locale": "en",
        "dht": true,
        "pex": true,
        "lsd": true,
        "encryption": 0,
        "queueing_enabled": true,
        "max_ratio_enabled": true,
        "max_ratio": 0,
        "max_ratio_act": 0,
        "max_seeding_time_enabled": false,
        "max_seeding_time": -1,
        "max_inactive_seeding_time_enabled": false,
        "max_inactive_seeding_time": -1,
        "add_trackers_enabled": false,
        "add_trackers": "",
        "incomplete_files_ext": false,
        "preallocate_all": false,
        "auto_tmm_enabled": false,
        "torrent_content_layout": "Original",
        "listen_port": 6881,
        "upnp": false,
        "dl_limit": 0,
        "up_limit": 0,
        "temp_path_enabled": false,
        "temp_path": "",
        "export_dir": "",
        "export_dir_fin": "",
    }))
}

pub async fn build_info() -> impl IntoResponse {
    Json(serde_json::json!({
        "qt": "6.7.0",
        "libtorrent": "2.0.10.0",
        "boost": "1.86.0",
        "openssl": "3.3.1",
        "zlib": "1.3.1",
        "bitness": 64,
    }))
}

pub async fn default_save_path(State(state): State<QbitState>) -> impl IntoResponse {
    let settings = state.manager.settings.read().await;
    let save_path = format!("{}/", settings.download_dir.trim_end_matches('/'));
    (StatusCode::OK, save_path)
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
