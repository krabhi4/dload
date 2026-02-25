pub mod download;
pub mod settings;
pub mod user;

pub use download::*;
pub use settings::*;
pub use user::*;

pub const JWT_SECRET: &[u8] = b"dload-secret-key-change-in-production";
