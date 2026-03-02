use std::sync::Arc;
use axum::{
    routing::get,
    response::{Html, IntoResponse},
    http::{header, StatusCode},
    Router,
};
use std::net::SocketAddr;
use axum::http::{Method, HeaderValue};
use tower_http::cors::CorsLayer;
use api::qbit_compat::session::SessionStore;

mod api;
mod db;
mod domain;
mod manager;
mod worker;

// Embed all UI assets at compile time — single binary, no external files needed
static INDEX_HTML: &str = include_str!("ui/index.html");
static STYLE_CSS: &str = include_str!("ui/style.css");
static APP_JS: &str = include_str!("ui/app.js");

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let db = Arc::new(db::Database::new("/data/dload.db").expect("Failed to create database"));
    let repo = Arc::new(db::repository::Repository::new(db.clone()));

    // Load settings from DB, falling back to defaults for first run.
    // We no longer rewrite user-chosen download_dir values (including `/data`);
    // whatever is stored in the DB is treated as authoritative.
    let settings = match repo.get_settings() {
        Ok(s) => s,
        Err(_) => {
            let defaults = domain::Settings::default();
            repo.save_settings(&defaults).ok();
            defaults
        }
    };

    let manager: manager::SharedState = Arc::new(manager::ManagerState::new(settings, repo));
    let sessions = Arc::new(SessionStore::new());

    let allowed_origin = std::env::var("DLOAD_CORS_ORIGIN").unwrap_or_default();
    let cors = if allowed_origin.is_empty() {
        // No CORS — only same-origin requests allowed (safest default)
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

    let app = Router::new()
        .route("/", get(index))
        .route("/ui/style.css", get(style_css))
        .route("/ui/app.js", get(app_js))
        .merge(api::router(manager.clone()))
        .merge(api::qbit_compat::router(manager, sessions))
        .layer(cors)
        .layer(security_headers);

    let addr = SocketAddr::from(([0, 0, 0, 0], 8080));
    tracing::info!("Server starting on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
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
