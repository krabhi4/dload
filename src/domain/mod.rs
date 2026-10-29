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
    match std::env::var("DLOAD_JWT_SECRET") {
        Ok(secret) if is_usable_secret(&secret) => secret,
        Ok(_) => {
            tracing::warn!("DLOAD_JWT_SECRET is a placeholder or shorter than 32 characters — ignoring it and using a generated secret");
            stored_jwt_secret()
        }
        Err(_) => stored_jwt_secret(),
    }
});

fn is_usable_secret(secret: &str) -> bool {
    secret.len() >= 32 && !secret.starts_with("changeme")
}

fn stored_jwt_secret() -> String {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let dir = std::env::var("DLOAD_DATA_DIR").unwrap_or_else(|_| "/data".to_string());
    let path = std::path::Path::new(&dir).join("jwt_secret");
    if let Ok(saved) = std::fs::read_to_string(&path) {
        let saved = saved.trim();
        if is_usable_secret(saved) {
            return saved.to_string();
        }
    }
    let secret = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let saved = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&path)
        .and_then(|mut f| f.write_all(secret.as_bytes()));
    if let Err(e) = saved {
        tracing::warn!(
            "Could not save generated JWT secret to {}: {} (sessions will end on restart)",
            path.display(),
            e
        );
    }
    secret
}

pub fn jwt_secret() -> &'static [u8] {
    JWT_SECRET_STRING.as_bytes()
}

static DUMMY_PASSWORD_HASH: LazyLock<String> = LazyLock::new(|| {
    bcrypt::hash(uuid::Uuid::new_v4().to_string(), 12).expect("bcrypt hash of a random string")
});

pub fn dummy_password_hash() -> &'static str {
    &DUMMY_PASSWORD_HASH
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
