use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub download_dir: String,
    pub max_concurrent: u32,
    pub max_connections_per_file: u32,
    pub chunk_size: u32,
    pub username: String,
    pub port: u16,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            download_dir: "/downloads".to_string(),
            max_concurrent: 3,
            max_connections_per_file: 4,
            chunk_size: 131072,
            username: "admin".to_string(),
            port: 8080,
        }
    }
}
