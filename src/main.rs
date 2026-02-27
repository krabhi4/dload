use std::sync::Arc;
use dload::api::qbit_compat::session::SessionStore;
use dload::{config, db, domain, manager, server};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let db = Arc::new(db::Database::new("/data/dload.db").expect("Failed to create database"));
    let repo = Arc::new(db::repository::Repository::new(db.clone()));

    let settings = match repo.get_settings() {
        Ok(mut s) => {
            if s.download_dir == "/data" {
                tracing::info!("Migrating download_dir from /data to /downloads");
                s.download_dir = "/downloads".to_string();
                repo.save_settings(&s).ok();
            }
            s
        }
        Err(_) => {
            let defaults = domain::Settings::default();
            repo.save_settings(&defaults).ok();
            defaults
        }
    };

    let manager: manager::SharedState = Arc::new(manager::ManagerState::new(settings, repo));
    let sessions = Arc::new(SessionStore::new());

    let allowed_origin = std::env::var("DLOAD_CORS_ORIGIN").unwrap_or_default();
    let cfg = config::RuntimeConfig::from_env();

    let app = server::build_app(manager, sessions, allowed_origin);

    let addr: std::net::SocketAddr = format!("{}:{}", cfg.host, cfg.port)
        .parse()
        .expect("Invalid bind address");
    tracing::info!("Server starting on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
