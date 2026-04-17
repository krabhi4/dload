use crate::domain::{jwt_secret, Claims};
use crate::manager::SharedState;
use axum::{
    extract::{Query, State},
    response::Json,
    routing::{get, post},
    Router,
};

fn extract_token(headers: &axum::http::HeaderMap) -> &str {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("")
}

fn require_admin(headers: &axum::http::HeaderMap) -> Result<Claims, String> {
    let token = extract_token(headers);
    jsonwebtoken::decode::<Claims>(
        token,
        &jsonwebtoken::DecodingKey::from_secret(jwt_secret()),
        &jsonwebtoken::Validation::default(),
    )
    .map(|d| d.claims)
    .map_err(|_| "Authentication required".to_string())
    .and_then(|c| {
        if c.role == "ADMIN" {
            Ok(c)
        } else {
            Err("Admin access required".to_string())
        }
    })
}

#[derive(serde::Deserialize)]
struct BrowseParams {
    path: Option<String>,
}

#[derive(serde::Serialize)]
struct BrowseEntry {
    name: String,
    path: String,
}

#[derive(serde::Serialize)]
struct BrowseResponse {
    current: String,
    parent: Option<String>,
    dirs: Vec<BrowseEntry>,
}

#[derive(serde::Deserialize)]
struct CreateDirRequest {
    path: String,
}

pub fn router(state: SharedState) -> Router {
    Router::new()
        .route("/api/browse", get(browse_dirs))
        .route("/api/browse/mkdir", post(create_dir))
        .with_state(state)
}

fn validate_browse_path(path: &str) -> Result<String, String> {
    let path = path.trim();
    if !path.starts_with('/') {
        return Err("Path must be absolute".to_string());
    }
    if path.contains("..") {
        return Err("Path must not contain '..'".to_string());
    }
    let blocked = [
        "/etc", "/usr", "/bin", "/sbin", "/boot", "/dev", "/proc", "/sys", "/var/run", "/root",
    ];
    for prefix in &blocked {
        if path == *prefix || path.starts_with(&format!("{}/", prefix)) {
            return Err("Cannot browse system directories".to_string());
        }
    }
    Ok(path.to_string())
}

async fn browse_dirs(
    Query(params): Query<BrowseParams>,
    headers: axum::http::HeaderMap,
    State(_state): State<SharedState>,
) -> axum::response::Response {
    if let Err(e) = require_admin(&headers) {
        return axum::response::IntoResponse::into_response((axum::http::StatusCode::FORBIDDEN, e));
    }

    let path = params.path.unwrap_or_else(|| "/".to_string());
    let path = match validate_browse_path(&path) {
        Ok(p) => p,
        Err(e) => {
            return axum::response::IntoResponse::into_response((
                axum::http::StatusCode::BAD_REQUEST,
                e,
            ));
        }
    };

    let mut dirs = Vec::new();
    match tokio::fs::read_dir(&path).await {
        Ok(mut entries) => {
            while let Ok(Some(entry)) = entries.next_entry().await {
                if let Ok(ft) = entry.file_type().await {
                    if ft.is_dir() {
                        let name = entry.file_name().to_string_lossy().to_string();
                        // Skip hidden directories
                        if name.starts_with('.') {
                            continue;
                        }
                        let full = format!("{}/{}", path.trim_end_matches('/'), name);
                        dirs.push(BrowseEntry { name, path: full });
                    }
                }
            }
        }
        Err(e) => {
            return axum::response::IntoResponse::into_response((
                axum::http::StatusCode::BAD_REQUEST,
                format!("Cannot read directory: {}", e),
            ));
        }
    }

    dirs.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

    let parent = if path != "/" {
        std::path::Path::new(&path)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .filter(|p| !p.is_empty())
            .or(Some("/".to_string()))
    } else {
        None
    };

    axum::response::IntoResponse::into_response(Json(BrowseResponse {
        current: path,
        parent,
        dirs,
    }))
}

async fn create_dir(
    headers: axum::http::HeaderMap,
    State(_state): State<SharedState>,
    Json(req): Json<CreateDirRequest>,
) -> axum::response::Response {
    if let Err(e) = require_admin(&headers) {
        return axum::response::IntoResponse::into_response((axum::http::StatusCode::FORBIDDEN, e));
    }

    let path = match validate_browse_path(&req.path) {
        Ok(p) => p,
        Err(e) => {
            return axum::response::IntoResponse::into_response((
                axum::http::StatusCode::BAD_REQUEST,
                e,
            ));
        }
    };

    // Walk existing ancestors; reject if any is a symlink — create_dir_all would
    // follow it and land outside the validated prefix.
    let mut cur = std::path::PathBuf::new();
    for component in std::path::Path::new(&path).components() {
        cur.push(component);
        match tokio::fs::symlink_metadata(&cur).await {
            Ok(md) if md.file_type().is_symlink() => {
                return axum::response::IntoResponse::into_response((
                    axum::http::StatusCode::BAD_REQUEST,
                    "Path traverses a symlink",
                ));
            }
            _ => {}
        }
    }

    match tokio::fs::create_dir_all(&path).await {
        Ok(_) => axum::response::IntoResponse::into_response(Json(
            serde_json::json!({ "success": true, "path": path }),
        )),
        Err(e) => axum::response::IntoResponse::into_response((
            axum::http::StatusCode::BAD_REQUEST,
            format!("Failed to create directory: {}", e),
        )),
    }
}
