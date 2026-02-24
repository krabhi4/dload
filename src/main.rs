use std::sync::Arc;
use axum::{
    routing::get,
    response::{Html, IntoResponse},
    http::{header, StatusCode},
    Router,
};
use std::net::SocketAddr;
use tower_http::cors::{Any, CorsLayer};

mod api;
mod db;
mod domain;
mod manager;
mod worker;

#[derive(Clone)]
struct AppState {
    manager: manager::SharedState,
}

// Embed all UI assets at compile time — single binary, no external files needed
static INDEX_HTML: &str = include_str!("ui/index.html");
static STYLE_CSS: &str = include_str!("ui/style.css");
static APP_JS: &str = include_str!("ui/app.js");

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let db = Arc::new(db::Database::new("/data/dload.db").expect("Failed to create database"));
    let repo = Arc::new(db::repository::Repository::new(db.clone()));

    // Load settings from DB, falling back to defaults for first run
    let settings = match repo.get_settings() {
        Ok(s) => s,
        Err(_) => {
            let defaults = domain::Settings::default();
            repo.save_settings(&defaults).ok();
            defaults
        }
    };

    let manager_state = manager::ManagerState::new(settings, repo);

    let state = Arc::new(AppState {
        manager: Arc::new(manager_state),
    });

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new()
        .route("/", get(index))
        .route("/ui/style.css", get(style_css))
        .route("/ui/app.js", get(app_js))
        .merge(api::router(state.manager.clone()))
        .layer(cors);

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
