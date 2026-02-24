use std::sync::Arc;
use axum::{
    routing::get,
    Router,
};
use std::net::SocketAddr;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::ServeDir;

mod api;
mod db;
mod domain;
mod manager;
mod worker;

#[derive(Clone)]
struct AppState {
    manager: manager::SharedState,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    
    let settings = domain::Settings::default();
    let db = Arc::new(db::Database::new("/data/dload.db").expect("Failed to create database"));
    
    let repo = db::repository::Repository::new(db.clone());
    repo.save_settings(&settings).ok();
    
    let manager_state = manager::ManagerState::new(settings);
    
    let state = Arc::new(AppState {
        manager: Arc::new(manager_state),
    });
    
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);
    
    let app = Router::new()
        .nest_service("/ui", ServeDir::new("src/ui"))
        .route("/", get(index))
        .merge(api::router(state.manager.clone()))
        .layer(cors);
    
    let addr = SocketAddr::from(([0, 0, 0, 0], 8080));
    tracing::info!("Server starting on {}", addr);
    
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn index() -> impl axum::response::IntoResponse {
    axum::response::Html(include_str!("ui/index.html"))
}
