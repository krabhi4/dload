use std::sync::Arc;
use axum::{
    Router,
    routing::get,
    response::{Html, IntoResponse},
    http::{header, StatusCode},
};
use axum::http::{Method, HeaderValue};
use tower_http::cors::CorsLayer;
use crate::api::qbit_compat::session::SessionStore;
use crate::manager::SharedState;

static INDEX_HTML: &str = include_str!("ui/index.html");
static STYLE_CSS: &str = include_str!("ui/style.css");
static APP_JS: &str = include_str!("ui/app.js");

pub fn build_app(
    manager: SharedState,
    sessions: Arc<SessionStore>,
    allowed_origin: String,
) -> Router {
    let cors = if allowed_origin.is_empty() {
        CorsLayer::new()
            .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
            .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE])
    } else {
        CorsLayer::new()
            .allow_origin(allowed_origin.parse::<HeaderValue>().expect("Invalid DLOAD_CORS_ORIGIN"))
            .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
            .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE])
    };

    let security_headers = axum::middleware::from_fn(security_headers_middleware);

    Router::new()
        .route("/", get(index))
        .route("/ui/style.css", get(style_css))
        .route("/ui/app.js", get(app_js))
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .merge(crate::api::router(manager.clone()))
        .merge(crate::api::qbit_compat::router(manager, sessions))
        .layer(cors)
        .layer(security_headers)
}

/// Build a minimal app instance for integration tests (in-memory SQLite, no active downloads).
pub async fn build_app_for_test() -> Router {
    use crate::db::Database;
    use crate::db::repository::Repository;
    use crate::domain::Settings;

    let db = Arc::new(Database::new(":memory:").expect("in-memory DB failed"));
    let repo = Arc::new(Repository::new(db));
    let settings = Settings::default();
    let _ = repo.save_settings(&settings);
    let manager: SharedState = Arc::new(crate::manager::ManagerState::new(settings, repo));
    let sessions = Arc::new(SessionStore::new());

    build_app(manager, sessions, String::new())
}

async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

async fn style_css() -> impl IntoResponse {
    (StatusCode::OK, [(header::CONTENT_TYPE, "text/css")], STYLE_CSS)
}

async fn app_js() -> impl IntoResponse {
    (StatusCode::OK, [(header::CONTENT_TYPE, "application/javascript")], APP_JS)
}

async fn healthz() -> impl IntoResponse {
    axum::Json(serde_json::json!({"status": "ok"}))
}

async fn readyz() -> impl IntoResponse {
    axum::Json(serde_json::json!({"status": "ready"}))
}

async fn security_headers_middleware(
    req: axum::http::Request<axum::body::Body>,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let mut resp = next.run(req).await;
    let headers = resp.headers_mut();
    headers.insert("X-Content-Type-Options", "nosniff".parse().unwrap());
    headers.insert("X-Frame-Options", "DENY".parse().unwrap());
    headers.insert("Referrer-Policy", "strict-origin-when-cross-origin".parse().unwrap());
    headers.insert("X-XSS-Protection", "1; mode=block".parse().unwrap());
    headers.insert(
        "Content-Security-Policy",
        "default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self'".parse().unwrap(),
    );
    resp
}
