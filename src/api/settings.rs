use crate::domain::Settings;
use crate::manager::SharedState;
use axum::{
    extract::State,
    response::Json,
    routing::{get, put},
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
    let mut current = state.settings.write().await;
    *current = settings;
    Json(serde_json::json!({ "success": true }))
}
