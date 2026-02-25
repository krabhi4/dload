pub mod download;
pub mod settings;
pub mod user;

pub use download::*;
pub use settings::*;
pub use user::*;

use std::sync::LazyLock;

static JWT_SECRET_STRING: LazyLock<String> = LazyLock::new(|| {
    std::env::var("DLOAD_JWT_SECRET").unwrap_or_else(|_| {
        tracing::warn!("DLOAD_JWT_SECRET not set — generating random secret (tokens will invalidate on restart)");
        uuid::Uuid::new_v4().to_string() + &uuid::Uuid::new_v4().to_string()
    })
});

pub fn jwt_secret() -> &'static [u8] {
    JWT_SECRET_STRING.as_bytes()
}
