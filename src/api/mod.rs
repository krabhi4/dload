pub mod auth;
pub mod browse;
pub mod downloads;
pub mod history;
pub mod torrents;
pub mod settings;
pub mod qbit_compat;

use crate::manager::SharedState;
use axum::Router;

pub fn router(state: SharedState) -> Router {
    Router::new()
        .merge(auth::router(state.clone()))
        .merge(browse::router(state.clone()))
        .merge(downloads::router(state.clone()))
        .merge(history::router(state.clone()))
        .merge(torrents::router(state.clone()))
        .merge(settings::router(state))
}
