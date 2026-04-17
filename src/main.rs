use axum::http::{HeaderValue, Method};
use axum::{
    http::{header, StatusCode},
    response::{Html, IntoResponse},
    routing::get,
    Router,
};
use std::net::SocketAddr;
use std::sync::Arc;
use tower_http::cors::CorsLayer;

mod api;
mod db;
mod domain;
mod manager;
mod worker;

// Embed all UI assets at compile time — single binary, no external files needed
static INDEX_HTML: &str = include_str!("ui/index.html");
static STYLE_CSS: &str = include_str!("ui/style.css");
static APP_JS: &str = include_str!("ui/app.js");
static FAVICON_SVG: &str = include_str!("ui/favicon.svg");
static MANIFEST_JSON: &str = include_str!("ui/manifest.json");

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let data_dir = std::env::var("DLOAD_DATA_DIR").unwrap_or_else(|_| "/data".to_string());
    std::fs::create_dir_all(&data_dir).expect("Failed to create data directory");
    let db_path = format!("{}/dload.db", data_dir);

    let db = Arc::new(db::Database::new(&db_path).expect("Failed to create database"));
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

    let (manager_state, auto_resume_ids) = manager::ManagerState::new(settings, repo);
    let manager: manager::SharedState = Arc::new(manager_state);
    let sessions = manager.sessions.clone();

    // Auto-resume torrents that were active before shutdown
    if !auto_resume_ids.is_empty() {
        tracing::info!(
            "{} torrents will auto-resume after startup",
            auto_resume_ids.len()
        );
        let mgr = manager.clone();
        tokio::spawn(async move {
            // Let server start and system settle before resuming
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            mgr.resume_all_downloads(auto_resume_ids, 3).await;
        });
    }

    // Periodic queue promotion: check every 2s for queued downloads that can start
    {
        let mgr = manager.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                mgr.try_start_queued().await;
            }
        });
    }

    let allowed_origin = std::env::var("DLOAD_CORS_ORIGIN").unwrap_or_default();
    let cors = if allowed_origin.is_empty() {
        // No CORS — only same-origin requests allowed (safest default)
        CorsLayer::new()
            .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
            .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE])
    } else {
        CorsLayer::new()
            .allow_origin(
                allowed_origin
                    .parse::<HeaderValue>()
                    .expect("Invalid DLOAD_CORS_ORIGIN"),
            )
            .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
            .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE])
    };

    let security_headers = axum::middleware::from_fn(security_headers_middleware);

    let app = Router::new()
        .route("/", get(index))
        .route("/ui/style.css", get(style_css))
        .route("/ui/app.js", get(app_js))
        .route("/ui/favicon.svg", get(favicon_svg))
        .route("/manifest.json", get(manifest_json))
        .merge(api::router(manager.clone()))
        .merge(api::qbit_compat::router(manager, sessions))
        .layer(cors)
        .layer(security_headers);

    let addr = SocketAddr::from(([0, 0, 0, 0], 8080));
    tracing::info!("Server starting on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .unwrap();
}

async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

async fn style_css() -> impl IntoResponse {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/css")],
        STYLE_CSS,
    )
}

async fn app_js() -> impl IntoResponse {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/javascript")],
        APP_JS,
    )
}

async fn favicon_svg() -> impl IntoResponse {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "image/svg+xml")],
        FAVICON_SVG,
    )
}

async fn manifest_json() -> impl IntoResponse {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/manifest+json")],
        MANIFEST_JSON,
    )
}

async fn security_headers_middleware(
    req: axum::http::Request<axum::body::Body>,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let mut resp = next.run(req).await;
    let headers = resp.headers_mut();
    headers.insert("X-Content-Type-Options", "nosniff".parse().unwrap());
    headers.insert("X-Frame-Options", "DENY".parse().unwrap());
    headers.insert(
        "Referrer-Policy",
        "strict-origin-when-cross-origin".parse().unwrap(),
    );
    headers.insert("X-XSS-Protection", "1; mode=block".parse().unwrap());
    headers.insert(
        "Content-Security-Policy",
        "default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self'".parse().unwrap(),
    );
    resp
}
