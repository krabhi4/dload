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
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Clone)]
pub struct QbitState {
    pub manager: SharedState,
    pub sessions: Arc<SessionStore>,
    pub categories: Arc<RwLock<HashSet<String>>>,
}

pub fn router(manager: SharedState, sessions: Arc<SessionStore>) -> Router {
    let state = QbitState { manager, sessions, categories: Arc::new(RwLock::new(HashSet::new())) };

    // Auth routes (no session required)
    let auth_routes = Router::new()
        .route("/api/v2/auth/login", post(auth::login))
        .route("/api/v2/auth/logout", post(auth::logout))
        .layer(middleware::from_fn(log_requests));

    // App routes (session required)
    let app_routes = Router::new()
        .route("/api/v2/app/version", get(app::version))
        .route("/api/v2/app/webapiVersion", get(app::webapi_version))
        .route("/api/v2/app/preferences", get(app::preferences))
        .route("/api/v2/app/buildInfo", get(app::build_info))
        .route("/api/v2/app/defaultSavePath", get(app::default_save_path));

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
        .route("/api/v2/torrents/tags", get(torrents::tags))
        .route("/api/v2/torrents/createCategory", post(torrents::create_category))
        .route("/api/v2/torrents/add", post(torrents::add))
        .route("/api/v2/torrents/delete", post(torrents::delete))
        .route("/api/v2/torrents/pause", post(torrents::pause))
        .route("/api/v2/torrents/resume", post(torrents::resume))
        .route("/api/v2/torrents/start", post(torrents::resume))
        .route("/api/v2/torrents/stop", post(torrents::pause))
        .route("/api/v2/torrents/setCategory", post(torrents::set_category))
        .route("/api/v2/torrents/setShareLimits", post(torrents::noop))
        .route("/api/v2/torrents/topPrio", post(torrents::noop))
        .route("/api/v2/torrents/setForceStart", post(torrents::noop))
        .route("/api/v2/torrents/bottomPrio", post(torrents::noop))
        .route("/api/v2/torrents/increasePrio", post(torrents::noop))
        .route("/api/v2/torrents/decreasePrio", post(torrents::noop))
        .route("/api/v2/torrents/setLocation", post(torrents::set_location))
        .route("/api/v2/torrents/rename", post(torrents::noop))
        .route("/api/v2/torrents/setSuperSeeding", post(torrents::noop))
        .route("/api/v2/torrents/setAutoManagement", post(torrents::noop))
        .route("/api/v2/torrents/addTags", post(torrents::noop))
        .route("/api/v2/torrents/removeTags", post(torrents::noop))
        .route("/api/v2/torrents/editCategory", post(torrents::noop))
        .route("/api/v2/torrents/removeCategories", post(torrents::noop))
        .route("/api/v2/torrents/removeCompleted", post(torrents::remove_completed))
        .route("/api/v2/torrents/recheck", post(torrents::noop))
        .route("/api/v2/torrents/reannounce", post(torrents::noop))
        .route("/api/v2/torrents/editTrackers", post(torrents::noop))
        .route("/api/v2/torrents/removeTrackers", post(torrents::noop))
        .route("/api/v2/torrents/addTrackers", post(torrents::noop))
        .route("/api/v2/torrents/addPeers", post(torrents::noop))
        .route("/api/v2/torrents/setDownloadLimit", post(torrents::set_download_limit))
        .route("/api/v2/torrents/setUploadLimit", post(torrents::set_upload_limit))
        .route("/api/v2/torrents/filePrio", post(torrents::file_prio))
        .route("/api/v2/torrents/toggleSequentialDownload", post(torrents::toggle_sequential_download))
        .route("/api/v2/torrents/toggleFirstLastPiecePrio", post(torrents::toggle_first_last_piece_prio));

    // Protected routes need session middleware
    let protected = app_routes
        .merge(transfer_routes)
        .merge(torrent_routes)
        .layer(middleware::from_fn_with_state(
            state.clone(),
            require_session,
        ))
        .layer(middleware::from_fn(log_requests));

    auth_routes
        .merge(protected)
        .with_state(state)
}

/// Log all qBit API requests with method, path, and response status for debugging.
async fn log_requests(
    request: axum::http::Request<axum::body::Body>,
    next: middleware::Next,
) -> impl IntoResponse {
    let method = request.method().clone();
    let uri = request.uri().clone();
    let response = next.run(request).await;
    let status = response.status();
    tracing::info!("qbit_compat {} {} → {}", method, uri, status.as_u16());
    response
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
        if let Some((_username, role)) = state.sessions.validate(&sid).await {
            if role == "ADMIN" {
                return next.run(request).await.into_response();
            }
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

    tracing::warn!("qbit_compat auth failed for {} {}", request.method(), request.uri());
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
    let (hash, is_admin) = match state.manager.repo.get_user_by_username(username) {
        Ok(Some(u)) => (u.password_hash, u.role == crate::domain::Role::Admin),
        _ => (DUMMY_HASH.to_string(), false),
    };
    bcrypt::verify(password, &hash).unwrap_or(false) && is_admin
}
