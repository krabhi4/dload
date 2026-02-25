use crate::domain::{Claims, jwt_secret};
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

fn require_auth(headers: &axum::http::HeaderMap) -> Result<Claims, String> {
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");

    jsonwebtoken::decode::<Claims>(
        token,
        &jsonwebtoken::DecodingKey::from_secret(jwt_secret()),
        &jsonwebtoken::Validation::default(),
    )
    .map(|d| d.claims)
    .map_err(|_| "Authentication required".to_string())
}

async fn list_torrents(
    State(state): State<SharedState>,
    headers: axum::http::HeaderMap,
) -> axum::response::Response {
    if require_auth(&headers).is_err() {
        return axum::response::IntoResponse::into_response(
            (axum::http::StatusCode::UNAUTHORIZED, "Authentication required")
        );
    }
    let downloads = state.get_all().await;
    let torrents: Vec<_> = downloads.into_iter()
        .filter(|d| d.protocol == crate::domain::Protocol::Torrent)
        .collect();
    axum::response::IntoResponse::into_response(Json(serde_json::json!({ "torrents": torrents })))
}
