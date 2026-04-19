use crate::domain::{jwt_secret, Claims, DownloadFolder, Settings};
use crate::manager::SharedState;
use axum::{extract::State, response::Json, routing::get, Router};

pub fn router(state: SharedState) -> Router {
    Router::new()
        .route("/api/settings", get(get_settings).put(update_settings))
        .with_state(state)
}

fn extract_token(headers: &axum::http::HeaderMap) -> &str {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("")
}

fn require_auth(headers: &axum::http::HeaderMap) -> Result<Claims, String> {
    jsonwebtoken::decode::<Claims>(
        extract_token(headers),
        &jsonwebtoken::DecodingKey::from_secret(jwt_secret()),
        &jsonwebtoken::Validation::default(),
    )
    .map(|d| d.claims)
    .map_err(|_| "Authentication required".to_string())
}

fn require_admin(headers: &axum::http::HeaderMap) -> Result<Claims, String> {
    require_auth(headers).and_then(|c| {
        if c.role == "ADMIN" {
            Ok(c)
        } else {
            Err("Admin access required".to_string())
        }
    })
}

async fn get_settings(
    State(state): State<SharedState>,
    headers: axum::http::HeaderMap,
) -> axum::response::Response {
    if require_auth(&headers).is_err() {
        return axum::response::IntoResponse::into_response((
            axum::http::StatusCode::UNAUTHORIZED,
            "Authentication required",
        ));
    }
    let settings = state.settings.read().await;
    axum::response::IntoResponse::into_response(Json(settings.clone()))
}

async fn update_settings(
    State(state): State<SharedState>,
    headers: axum::http::HeaderMap,
    Json(settings): Json<Settings>,
) -> axum::response::Response {
    if let Err(e) = require_admin(&headers) {
        return axum::response::IntoResponse::into_response((axum::http::StatusCode::FORBIDDEN, e));
    }

    if settings.max_concurrent == 0 {
        return axum::response::IntoResponse::into_response((
            axum::http::StatusCode::BAD_REQUEST,
            "max_concurrent must be at least 1",
        ));
    }
    if settings.max_connections_per_file == 0 {
        return axum::response::IntoResponse::into_response((
            axum::http::StatusCode::BAD_REQUEST,
            "max_connections_per_file must be at least 1",
        ));
    }

    // Validate download_dir
    if let Err(e) = validate_download_path(settings.download_dir.trim()) {
        return axum::response::IntoResponse::into_response((
            axum::http::StatusCode::BAD_REQUEST,
            e,
        ));
    }

    // If old client omitted download_folders, generate from download_dir
    let mut settings = settings;
    if settings.download_folders.is_empty() {
        settings.download_folders = vec![DownloadFolder {
            id: uuid::Uuid::new_v4().to_string(),
            label: "Default".to_string(),
            path: settings.download_dir.clone(),
            is_default: true,
        }];
    }

    // Validate download_folders
    if let Err(e) = validate_folders(&settings.download_folders) {
        return axum::response::IntoResponse::into_response((
            axum::http::StatusCode::BAD_REQUEST,
            e,
        ));
    }
    if let Some(default_folder) = settings.download_folders.iter().find(|f| f.is_default) {
        settings.download_dir = default_folder.path.clone();
    }

    let settings_to_save = settings.clone();
    if let Err(e) = state
        .repo_blocking(move |repo| repo.save_settings(&settings_to_save))
        .await
    {
        tracing::error!("Failed to persist settings to DB: {}", e);
    }
    let mut current = state.settings.write().await;
    *current = settings;
    axum::response::IntoResponse::into_response(Json(serde_json::json!({ "success": true })))
}

pub fn validate_download_path(dir: &str) -> Result<(), &'static str> {
    if !dir.starts_with('/') {
        return Err("Download directory must be an absolute path");
    }
    let blocked_prefixes = [
        "/etc", "/usr", "/bin", "/sbin", "/boot", "/dev", "/proc", "/sys", "/var/run", "/root",
    ];
    for prefix in &blocked_prefixes {
        if dir == *prefix || dir.starts_with(&format!("{}/", prefix)) {
            return Err("Download directory cannot be a system directory");
        }
    }
    if dir.contains("..") {
        return Err("Download directory must not contain '..'");
    }
    Ok(())
}

fn validate_folders(folders: &[DownloadFolder]) -> Result<(), &'static str> {
    if folders.is_empty() {
        return Err("At least one download folder is required");
    }
    let default_count = folders.iter().filter(|f| f.is_default).count();
    if default_count != 1 {
        return Err("Exactly one folder must be set as default");
    }
    for f in folders {
        if f.label.trim().is_empty() {
            return Err("Folder label must not be empty");
        }
        validate_download_path(f.path.trim())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(id: &str, label: &str, path: &str, is_default: bool) -> DownloadFolder {
        DownloadFolder {
            id: id.into(),
            label: label.into(),
            path: path.into(),
            is_default,
        }
    }

    // ─── validate_download_path ──────────────────────────────────────

    #[test]
    fn path_validation_accepts_absolute_paths() {
        assert!(validate_download_path("/downloads").is_ok());
        assert!(validate_download_path("/media/tv").is_ok());
        assert!(validate_download_path("/mnt/nas/movies").is_ok());
    }

    #[test]
    fn path_validation_rejects_relative_paths() {
        assert!(validate_download_path("downloads").is_err());
        assert!(validate_download_path("./downloads").is_err());
    }

    #[test]
    fn path_validation_rejects_system_directories() {
        for dir in [
            "/etc", "/usr", "/bin", "/sbin", "/boot", "/dev", "/proc", "/sys", "/root",
        ] {
            assert!(
                validate_download_path(dir).is_err(),
                "{dir} should be rejected"
            );
            let subdir = format!("{dir}/subdir");
            assert!(
                validate_download_path(&subdir).is_err(),
                "{subdir} should be rejected"
            );
        }
    }

    #[test]
    fn path_validation_rejects_traversal() {
        assert!(validate_download_path("/downloads/../etc").is_err());
        assert!(validate_download_path("/downloads/..").is_err());
    }

    // ─── validate_folders ────────────────────────────────────────────

    #[test]
    fn folders_validation_accepts_valid_set() {
        let folders = vec![
            folder("a", "Default", "/downloads", true),
            folder("b", "TV", "/media/tv", false),
        ];
        assert!(validate_folders(&folders).is_ok());
    }

    #[test]
    fn folders_validation_rejects_empty() {
        assert!(validate_folders(&[]).is_err());
    }

    #[test]
    fn folders_validation_rejects_no_default() {
        let folders = vec![
            folder("a", "Downloads", "/downloads", false),
            folder("b", "TV", "/media/tv", false),
        ];
        assert!(validate_folders(&folders).is_err());
    }

    #[test]
    fn folders_validation_rejects_multiple_defaults() {
        let folders = vec![
            folder("a", "Downloads", "/downloads", true),
            folder("b", "TV", "/media/tv", true),
        ];
        assert!(validate_folders(&folders).is_err());
    }

    #[test]
    fn folders_validation_rejects_empty_label() {
        let folders = vec![folder("a", "", "/downloads", true)];
        assert!(validate_folders(&folders).is_err());
    }

    #[test]
    fn folders_validation_rejects_system_path() {
        let folders = vec![folder("a", "Bad", "/etc", true)];
        assert!(validate_folders(&folders).is_err());
    }
}
