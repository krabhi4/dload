use crate::manager::SharedState;
use axum::{
    extract::State,
    response::Json,
    routing::get,
    Router,
};

pub fn router(state: SharedState) -> Router {
    Router::new()
        .route("/api/torrents", get(list_torrents))
        .with_state(state)
}

async fn list_torrents(State(state): State<SharedState>) -> Json<serde_json::Value> {
    let downloads = state.get_all().await;
    let torrents: Vec<_> = downloads.into_iter()
        .filter(|d| d.protocol == crate::domain::Protocol::Torrent)
        .collect();
    Json(serde_json::json!({ "torrents": torrents }))
}
