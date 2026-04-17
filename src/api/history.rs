use crate::domain::{jwt_secret, Claims};
use crate::manager::SharedState;
use axum::{
    extract::{Path, Query, State},
    response::Json,
    routing::{delete, get},
    Router,
};

#[derive(serde::Deserialize, Default)]
struct HistoryQuery {
    limit: Option<i64>,
    offset: Option<i64>,
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
        &jsonwebtoken::DecodingKey::from_secret(jwt_secret()),
        &jsonwebtoken::Validation::default(),
    )
    .map(|d| d.claims)
    .map_err(|_| "Authentication required".to_string())
}

pub fn router(state: SharedState) -> Router {
    Router::new()
        .route("/api/history", get(list_history).delete(clear_history))
        .route("/api/history/:id", delete(delete_history_item))
        .with_state(state)
}

async fn list_history(
    State(state): State<SharedState>,
    Query(q): Query<HistoryQuery>,
    headers: axum::http::HeaderMap,
) -> axum::response::Response {
    if require_auth(extract_token(&headers)).is_err() {
        return axum::response::IntoResponse::into_response((
            axum::http::StatusCode::UNAUTHORIZED,
            "Authentication required",
        ));
    }
    let limit = q.limit.unwrap_or(100).clamp(1, 1000);
    let offset = q.offset.unwrap_or(0).max(0);
    axum::response::IntoResponse::into_response(Json(state.get_history_page(limit, offset).await))
}

async fn delete_history_item(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    headers: axum::http::HeaderMap,
) -> Json<serde_json::Value> {
    if require_auth(extract_token(&headers)).is_err() {
        return Json(serde_json::json!({ "success": false, "error": "Authentication required" }));
    }
    state.delete_history(&id).await;
    Json(serde_json::json!({ "success": true }))
}

async fn clear_history(
    State(state): State<SharedState>,
    headers: axum::http::HeaderMap,
) -> Json<serde_json::Value> {
    if require_auth(extract_token(&headers)).is_err() {
        return Json(serde_json::json!({ "success": false, "error": "Authentication required" }));
    }
    state.delete_all_history().await;
    Json(serde_json::json!({ "success": true }))
}
