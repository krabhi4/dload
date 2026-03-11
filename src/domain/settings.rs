use serde::{Deserialize, Serialize};

fn default_min_split_size() -> u32 {
    20 * 1024 * 1024
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub download_dir: String,
    pub max_concurrent: u32,
    pub max_connections_per_file: u32,
    #[serde(default = "default_min_split_size", alias = "chunk_size")]
    pub min_split_size: u32,
    pub username: String,
    pub port: u16,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            download_dir: "/downloads".to_string(),
            max_concurrent: 3,
            max_connections_per_file: 8,
            min_split_size: 20 * 1024 * 1024,
            username: "admin".to_string(),
            port: 8080,
        }
    }
}
