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

    // Torrent routes (session required)
    let torrent_routes = Router::new()
        .route("/api/v2/torrents/info", get(torrents::info))
        .route("/api/v2/torrents/properties", get(torrents::properties))
        .route("/api/v2/torrents/files", get(torrents::files))
        .route("/api/v2/torrents/trackers", get(torrents::trackers))
        .route("/api/v2/torrents/categories", get(torrents::categories))
        .route("/api/v2/torrents/add", post(torrents::add))
        .route("/api/v2/torrents/delete", post(torrents::delete))
        .route("/api/v2/torrents/pause", post(torrents::pause))
        .route("/api/v2/torrents/resume", post(torrents::resume))
        .route("/api/v2/torrents/start", post(torrents::resume))
        .route("/api/v2/torrents/stop", post(torrents::pause))
        .route("/api/v2/torrents/setCategory", post(torrents::set_category));

    // Protected routes need session middleware
    let protected = app_routes
        .merge(torrent_routes)
        .layer(middleware::from_fn_with_state(
            state.clone(),
            require_session,
        ));

    auth_routes
        .merge(protected)
        .with_state(state)
}

async fn require_session(
    axum::extract::State(state): axum::extract::State<QbitState>,
    headers: axum::http::HeaderMap,
    request: axum::http::Request<axum::body::Body>,
    next: middleware::Next,
) -> impl IntoResponse {
    let sid = auth::extract_sid(&headers);

    match sid {
        Some(sid) if state.sessions.validate(&sid).await.is_some() => {
            next.run(request).await.into_response()
        }
        _ => (StatusCode::FORBIDDEN, "Forbidden").into_response(),
    }
}
