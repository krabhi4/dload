use crate::domain::{Claims, Settings, JWT_SECRET};
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

fn require_auth(headers: &axum::http::HeaderMap) -> Result<Claims, String> {
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");

    jsonwebtoken::decode::<Claims>(
        token,
        &jsonwebtoken::DecodingKey::from_secret(JWT_SECRET),
        &jsonwebtoken::Validation::default(),
    )
    .map(|d| d.claims)
    .map_err(|_| "Authentication required".to_string())
}

async fn get_settings(
    State(state): State<SharedState>,
    headers: axum::http::HeaderMap,
) -> axum::response::Response {
    if require_auth(&headers).is_err() {
        return axum::response::IntoResponse::into_response(
            (axum::http::StatusCode::UNAUTHORIZED, "Authentication required")
        );
    }
    let settings = state.settings.read().await;
    axum::response::IntoResponse::into_response(Json(settings.clone()))
}

async fn update_settings(
    State(state): State<SharedState>,
    headers: axum::http::HeaderMap,
    Json(settings): Json<Settings>,
) -> axum::response::Response {
    if require_auth(&headers).is_err() {
        return axum::response::IntoResponse::into_response(
            (axum::http::StatusCode::UNAUTHORIZED, "Authentication required")
        );
    }
    if let Err(e) = state.repo.save_settings(&settings) {
        tracing::error!("Failed to persist settings to DB: {}", e);
    }
    let mut current = state.settings.write().await;
    *current = settings;
    axum::response::IntoResponse::into_response(Json(serde_json::json!({ "success": true })))
}
