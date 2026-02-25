pub mod auth;
pub mod app;
pub mod session;
pub mod torrents;

use crate::manager::SharedState;
use axum::{
    Router,
    routing::{get, post},
    middleware,
    http::StatusCode,
    response::IntoResponse,
};
use session::SessionStore;
use std::sync::Arc;

#[derive(Clone)]
pub struct QbitState {
    pub manager: SharedState,
    pub sessions: Arc<SessionStore>,
}

pub fn router(manager: SharedState, sessions: Arc<SessionStore>) -> Router {
    let state = QbitState { manager, sessions };

    // Auth routes (no session required)
    let auth_routes = Router::new()
        .route("/api/v2/auth/login", post(auth::login))
        .route("/api/v2/auth/logout", post(auth::logout));

    // App routes (session required)
    let app_routes = Router::new()
        .route("/api/v2/app/version", get(app::version))
        .route("/api/v2/app/webapiVersion", get(app::webapi_version))
        .route("/api/v2/app/preferences", get(app::preferences));

    // Transfer routes (session required)
    let transfer_routes = Router::new()
        .route("/api/v2/transfer/info", get(app::transfer_info));

    // Torrent routes (session required)
    let torrent_routes = Router::new()
        .route("/api/v2/torrents/info", get(torrents::info))
        .route("/api/v2/torrents/properties", get(torrents::properties))
        .route("/api/v2/torrents/files", get(torrents::files))
        .route("/api/v2/torrents/trackers", get(torrents::trackers))
        .route("/api/v2/torrents/categories", get(torrents::categories))
        .route("/api/v2/torrents/createCategory", post(torrents::create_category))
        .route("/api/v2/torrents/add", post(torrents::add))
        .route("/api/v2/torrents/delete", post(torrents::delete))
        .route("/api/v2/torrents/pause", post(torrents::pause))
        .route("/api/v2/torrents/resume", post(torrents::resume))
        .route("/api/v2/torrents/start", post(torrents::resume))
        .route("/api/v2/torrents/stop", post(torrents::pause))
        .route("/api/v2/torrents/setCategory", post(torrents::set_category));

    // Protected routes need session middleware
    let protected = app_routes
        .merge(transfer_routes)
        .merge(torrent_routes)
        .layer(middleware::from_fn_with_state(
            state.clone(),
            require_session,
        ));

    auth_routes
        .merge(protected)
        .with_state(state)
}

/// Authenticate via SID cookie or Basic Auth fallback.
/// Sonarr sends NetworkCredential (Basic Auth) on every request in addition to cookies.
/// Some HTTP clients don't properly resend Set-Cookie values, so Basic Auth ensures compat.
async fn require_session(
    axum::extract::State(state): axum::extract::State<QbitState>,
    headers: axum::http::HeaderMap,
    request: axum::http::Request<axum::body::Body>,
    next: middleware::Next,
) -> impl IntoResponse {
    // Try SID cookie first
    if let Some(sid) = auth::extract_sid(&headers) {
        if state.sessions.validate(&sid).await.is_some() {
            return next.run(request).await.into_response();
        }
    }

    // Fallback: Basic Auth header
    if let Some(auth_header) = headers.get(axum::http::header::AUTHORIZATION) {
        if let Ok(auth_str) = auth_header.to_str() {
            if let Some(encoded) = auth_str.strip_prefix("Basic ") {
                if let Ok(decoded) = base64_decode(encoded) {
                    if let Some((username, password)) = decoded.split_once(':') {
                        let state_clone = state.clone();
                        let username = username.to_string();
                        let password = password.to_string();
                        let ok = tokio::task::spawn_blocking(move || {
                            validate_credentials(&state_clone, &username, &password)
                        })
                        .await
                        .unwrap_or(false);
                        if ok {
                            return next.run(request).await.into_response();
                        }
                    }
                }
            }
        }
    }

    (StatusCode::FORBIDDEN, "Forbidden").into_response()
}

fn base64_decode(input: &str) -> Result<String, ()> {
    use base64::{Engine, engine::general_purpose};
    let bytes = general_purpose::STANDARD
        .decode(input.trim())
        .or_else(|_| general_purpose::URL_SAFE.decode(input.trim()))
        .map_err(|_| ())?;
    String::from_utf8(bytes).map_err(|_| ())
}

fn validate_credentials(state: &QbitState, username: &str, password: &str) -> bool {
    // Always run bcrypt to prevent timing oracle on username existence
    const DUMMY_HASH: &str = "$2b$12$000000000000000000000uGKWMKFz95uGKWMKFz95uGKWMKFz9.";
    let hash = match state.manager.repo.get_user_by_username(username) {
        Ok(Some(u)) => u.password_hash,
        _ => DUMMY_HASH.to_string(),
    };
    bcrypt::verify(password, &hash).unwrap_or(false)
}
