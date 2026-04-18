use serde::{Deserialize, Serialize};

fn default_min_split_size() -> u32 {
    20 * 1024 * 1024
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadFolder {
    pub id: String,
    pub label: String,
    pub path: String,
    pub is_default: bool,
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
    #[serde(default)]
    pub download_folders: Vec<DownloadFolder>,
}

impl Settings {
    pub fn default_folder_path(&self) -> &str {
        self.download_folders
            .iter()
            .find(|f| f.is_default)
            .map(|f| f.path.as_str())
            .unwrap_or(&self.download_dir)
    }

    pub fn folder_path_by_id(&self, id: &str) -> Option<&str> {
        self.download_folders
            .iter()
            .find(|f| f.id == id)
            .map(|f| f.path.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_folders() -> Vec<DownloadFolder> {
        vec![
            DownloadFolder {
                id: "a".into(),
                label: "Default".into(),
                path: "/downloads".into(),
                is_default: true,
            },
            DownloadFolder {
                id: "b".into(),
                label: "TV".into(),
                path: "/media/tv".into(),
                is_default: false,
            },
            DownloadFolder {
                id: "c".into(),
                label: "Movies".into(),
                path: "/media/movies".into(),
                is_default: false,
            },
        ]
    }

    #[test]
    fn default_folder_path_returns_default_folder() {
        let s = Settings {
            download_folders: make_folders(),
            ..Settings::default()
        };
        assert_eq!(s.default_folder_path(), "/downloads");
    }

    #[test]
    fn default_folder_path_falls_back_to_download_dir() {
        let s = Settings {
            download_folders: vec![],
            ..Settings::default()
        };
        assert_eq!(s.default_folder_path(), "/downloads");
    }

    #[test]
    fn folder_path_by_id_finds_matching() {
        let s = Settings {
            download_folders: make_folders(),
            ..Settings::default()
        };
        assert_eq!(s.folder_path_by_id("b"), Some("/media/tv"));
        assert_eq!(s.folder_path_by_id("c"), Some("/media/movies"));
    }

    #[test]
    fn folder_path_by_id_returns_none_for_missing() {
        let s = Settings {
            download_folders: make_folders(),
            ..Settings::default()
        };
        assert_eq!(s.folder_path_by_id("nonexistent"), None);
    }

    #[test]
    fn default_settings_has_one_default_folder() {
        let s = Settings::default();
        assert_eq!(s.download_folders.len(), 1);
        assert!(s.download_folders[0].is_default);
        assert_eq!(s.download_folders[0].path, "/downloads");
        assert_eq!(s.download_folders[0].label, "Default");
    }
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
            download_folders: vec![DownloadFolder {
                id: uuid::Uuid::new_v4().to_string(),
                label: "Default".to_string(),
                path: "/downloads".to_string(),
                is_default: true,
            }],
        }
    }
}
