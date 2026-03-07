use crate::domain::{jwt_secret, Claims, Settings};
use crate::manager::SharedState;
use axum::{extract::State, response::Json, routing::get, Router};

pub fn router(state: SharedState) -> Router {
    Router::new()
        .route("/api/settings", get(get_settings).put(update_settings))
        .with_state(state)
}

fn extract_token(headers: &axum::http::HeaderMap) -> &str {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("")
}

fn require_auth(headers: &axum::http::HeaderMap) -> Result<Claims, String> {
    jsonwebtoken::decode::<Claims>(
        extract_token(headers),
        &jsonwebtoken::DecodingKey::from_secret(jwt_secret()),
        &jsonwebtoken::Validation::default(),
    )
    .map(|d| d.claims)
    .map_err(|_| "Authentication required".to_string())
}

fn require_admin(headers: &axum::http::HeaderMap) -> Result<Claims, String> {
    require_auth(headers).and_then(|c| {
        if c.role == "ADMIN" {
            Ok(c)
        } else {
            Err("Admin access required".to_string())
        }
    })
}

async fn get_settings(
    State(state): State<SharedState>,
    headers: axum::http::HeaderMap,
) -> axum::response::Response {
    if require_auth(&headers).is_err() {
        return axum::response::IntoResponse::into_response((
            axum::http::StatusCode::UNAUTHORIZED,
            "Authentication required",
        ));
    }
    let settings = state.settings.read().await;
    axum::response::IntoResponse::into_response(Json(settings.clone()))
}

async fn update_settings(
    State(state): State<SharedState>,
    headers: axum::http::HeaderMap,
    Json(settings): Json<Settings>,
) -> axum::response::Response {
    if let Err(e) = require_admin(&headers) {
        return axum::response::IntoResponse::into_response((axum::http::StatusCode::FORBIDDEN, e));
    }

    // Validate download_dir — must be absolute and not a system directory
    let dir = settings.download_dir.trim();
    if !dir.starts_with('/') {
        return axum::response::IntoResponse::into_response((
            axum::http::StatusCode::BAD_REQUEST,
            "Download directory must be an absolute path",
        ));
    }
    let blocked_prefixes = [
        "/etc", "/usr", "/bin", "/sbin", "/boot", "/dev", "/proc", "/sys", "/var/run", "/root",
    ];
    for prefix in &blocked_prefixes {
        if dir == *prefix || dir.starts_with(&format!("{}/", prefix)) {
            return axum::response::IntoResponse::into_response((
                axum::http::StatusCode::BAD_REQUEST,
                "Download directory cannot be a system directory",
            ));
        }
    }
    if dir.contains("..") {
        return axum::response::IntoResponse::into_response((
            axum::http::StatusCode::BAD_REQUEST,
            "Download directory must not contain '..'",
        ));
    }

    if let Err(e) = state.repo.save_settings(&settings) {
        tracing::error!("Failed to persist settings to DB: {}", e);
    }
    let mut current = state.settings.write().await;
    *current = settings;
    axum::response::IntoResponse::into_response(Json(serde_json::json!({ "success": true })))
}
