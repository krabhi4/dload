pub mod download;
pub mod settings;
pub mod torrent_layout;
pub mod user;

pub use download::*;
pub use settings::*;
pub use torrent_layout::*;
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

/// Prefix that distinguishes an API key from a JWT (JWTs never start with it).
pub const API_KEY_PREFIX: &str = "dload_";

/// Prefix + 256 bits of CSPRNG entropy (two v4 UUIDs). Shown once, never stored.
pub fn generate_api_key() -> String {
    format!(
        "{}{}{}",
        API_KEY_PREFIX,
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

// Full-entropy key → a single SHA-256 is enough; only this hash is stored.
pub fn hash_api_key(key: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(key.as_bytes()))
}
