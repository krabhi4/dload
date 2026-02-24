use crate::domain::Settings;
use crate::manager::SharedState;
use axum::{
    extract::State,
    response::Json,
    routing::get,
    Router,
};

pub fn router(state: SharedState) -> Router {
    Router::new()
        .route("/api/settings", get(get_settings).put(update_settings))
        .with_state(state)
}

async fn get_settings(State(state): State<SharedState>) -> Json<Settings> {
    let settings = state.settings.read().await;
    Json(settings.clone())
}

async fn update_settings(
    State(state): State<SharedState>,
    Json(settings): Json<Settings>,
) -> Json<serde_json::Value> {
    if let Err(e) = state.repo.save_settings(&settings) {
        tracing::error!("Failed to persist settings to DB: {}", e);
    }
    let mut current = state.settings.write().await;
    *current = settings;
    Json(serde_json::json!({ "success": true }))
}
