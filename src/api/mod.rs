pub mod auth;
pub mod downloads;
pub mod torrents;
pub mod settings;

use crate::manager::SharedState;
use axum::Router;

pub fn router(state: SharedState) -> Router {
    Router::new()
        .merge(auth::router(state.clone()))
        .merge(downloads::router(state.clone()))
        .merge(torrents::router(state.clone()))
        .merge(settings::router(state))
}
