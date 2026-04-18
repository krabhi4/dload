use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Deserialize;

use super::QbitState;
use crate::domain::{Download, DownloadStatus, Protocol};

/// No-op handler for endpoints we accept but don't act on (setShareLimits, topPrio, etc.)
pub async fn noop(_body: String) -> impl IntoResponse {
    (StatusCode::OK, "Ok.")
}

/// Resolve download directory for a qBit add request.
/// Priority: category→folder mapping > savepath matching a folder > default folder.
async fn resolve_qbit_download_dir(
    state: &QbitState,
    category: Option<&str>,
    savepath: Option<&str>,
) -> String {
    let settings = state.manager.settings.read().await;
    let default_path = settings.default_folder_path().to_string();

    // 1. Check if category maps to a folder
    if let Some(cat) = category {
        let cats = state.categories.read().await;
        if let Some(Some(folder_id)) = cats.get(cat) {
            if let Some(path) = settings.folder_path_by_id(folder_id) {
                return path.to_string();
            }
        }
    }

    // 2. Check if savepath matches a configured folder
    if let Some(sp) = savepath {
        let sp = sp.trim_end_matches('/');
        if !sp.is_empty() {
            for f in &settings.download_folders {
                let fp = f.path.trim_end_matches('/');
                if sp == fp || sp.starts_with(&format!("{}/", fp)) {
                    return sp.to_string();
                }
            }
            tracing::warn!(
                "qbit_compat: savepath '{}' doesn't match any configured folder, using default",
                sp
            );
        }
    }

    default_path
}

/// Case-insensitive hash match (Sonarr sends lowercase, some clients may differ)
fn hash_matches(download: &Download, hash: &str) -> bool {
    download
        .info_hash
        .as_deref()
        .map(|h| h.eq_ignore_ascii_case(hash))
        .unwrap_or(false)
        || download.id == hash
}

/// Map dload DownloadStatus to qBittorrent state string.
/// Both Seeding and Completed map to "pausedUP" because:
/// 1. dload declares max_ratio=0 in preferences (no seeding goal)
/// 2. Sonarr/Radarr require "pausedUP" or "stoppedUP" for CanMoveFiles=true
/// 3. This enables auto-import without requiring manual intervention
fn to_qbit_state(status: &DownloadStatus) -> &'static str {
    match status {
        DownloadStatus::Queued => "queuedDL",
        DownloadStatus::Downloading => "downloading",
        DownloadStatus::Paused => "pausedDL",
        DownloadStatus::Completed => "pausedUP",
        DownloadStatus::Failed => "error",
        DownloadStatus::Stopped => "pausedDL",
        DownloadStatus::Seeding => "pausedUP",
    }
}

/// Convert a Download to the qBittorrent torrent info JSON format
fn to_qbit_torrent(d: &Download) -> serde_json::Value {
    let added_on = d.created_at.timestamp();
    let completion_on = d.completed_at.map(|c| c.timestamp()).unwrap_or(-1);

    // Parse ETA string to seconds, or 8640000 (infinity) if unavailable
    let eta_secs = d
        .eta
        .as_ref()
        .map(|e| parse_eta_to_secs(e))
        .unwrap_or(8640000);

    let content_path = d.content_path.as_deref().unwrap_or(&d.save_path);
    let save_path = format!(
        "{}/",
        std::path::Path::new(&d.save_path)
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(|p| p.to_string_lossy())
            .unwrap_or(std::borrow::Cow::Borrowed(&d.save_path))
    );

    let now = chrono::Utc::now();
    let time_active = (now - d.created_at).num_seconds();
    let is_finished = d.status == DownloadStatus::Seeding || d.status == DownloadStatus::Completed;
    let seeding_time = if is_finished {
        d.completed_at.map(|c| (now - c).num_seconds()).unwrap_or(0)
    } else {
        0
    };

    let progress = if is_finished || d.progress >= 100.0 {
        1.0
    } else {
        d.progress / 100.0
    };
    let magnet_uri = if d.url.starts_with("magnet:") {
        d.url.as_str()
    } else {
        ""
    };

    let mut base = serde_json::json!({
        "hash": d.info_hash.as_deref().unwrap_or(&d.id),
        "name": d.filename,
        "size": d.total_size,
        "progress": progress,
        "dlspeed": d.speed,
        "upspeed": d.upload_speed,
        "state": to_qbit_state(&d.status),
        "category": d.category.as_deref().unwrap_or(""),
        "label": d.category.as_deref().unwrap_or(""),
        "tags": "",
        "save_path": save_path,
        "content_path": content_path,
        "added_on": added_on,
        "completion_on": completion_on,
        "eta": eta_secs,
        "num_seeds": d.seeds,
        "num_leechs": d.peers,
    });

    let amount_left = if is_finished || progress >= 1.0 {
        0
    } else {
        d.total_size.saturating_sub(d.downloaded_size)
    };
    let completed = if is_finished || progress >= 1.0 {
        d.total_size
    } else {
        d.downloaded_size
    };

    let extra = serde_json::json!({
        "num_complete": d.seeds,
        "num_incomplete": d.peers,
        "ratio": 0.0,
        "ratio_limit": -2,
        "seeding_time": seeding_time,
        "seeding_time_limit": -2,
        "inactive_seeding_time_limit": -2,
        "last_activity": now.timestamp(),
        "downloaded": d.downloaded_size,
        "uploaded": 0,
        "dl_limit": -1,
        "up_limit": -1,
        "amount_left": amount_left,
        "completed": completed,
        "total_size": d.total_size,
        "time_active": time_active,
        "tracker": "",
        "magnet_uri": magnet_uri,
        "priority": 0,
        "seq_dl": false,
        "f_l_piece_prio": false,
        "auto_tmm": false,
        "availability": if progress >= 1.0 { -1.0 } else { progress },
        "force_start": false,
        "max_ratio": -1,
        "max_seeding_time": -1,
        "max_inactive_seeding_time": -1,
        "seen_complete": completion_on,
        "downloaded_session": d.downloaded_size,
        "uploaded_session": 0,
    });

    if let (Some(base_map), Some(extra_map)) = (base.as_object_mut(), extra.as_object()) {
        for (k, v) in extra_map {
            base_map.insert(k.clone(), v.clone());
        }
    }
    base
}

fn parse_eta_to_secs(eta: &str) -> i64 {
    let mut secs: i64 = 0;
    let mut num = String::new();
    for c in eta.chars() {
        if c.is_ascii_digit() {
            num.push(c);
        } else {
            let n: i64 = num.parse().unwrap_or(0);
            num.clear();
            match c {
                'h' => secs += n * 3600,
                'm' => secs += n * 60,
                's' => secs += n,
                _ => {}
            }
        }
    }
    // Flush trailing digits without a unit suffix as seconds
    if let Ok(n) = num.parse::<i64>() {
        secs += n;
    }
    secs
}

#[derive(Deserialize)]
#[allow(dead_code)]
pub struct InfoQuery {
    pub filter: Option<String>,
    pub category: Option<String>,
    pub hashes: Option<String>,
    pub sort: Option<String>,
    pub reverse: Option<bool>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

pub async fn info(
    State(state): State<QbitState>,
    Query(query): Query<InfoQuery>,
) -> impl IntoResponse {
    let all = state.manager.get_all().await;

    let mut torrents: Vec<&Download> = all
        .iter()
        .filter(|d| d.protocol == Protocol::Torrent && d.info_hash.is_some())
        .collect();

    // Filter by hashes (case-insensitive)
    if let Some(ref hashes) = query.hashes {
        let hash_set: Vec<String> = hashes.split('|').map(|h| h.to_ascii_lowercase()).collect();
        torrents.retain(|d| {
            d.info_hash
                .as_deref()
                .map(|h| hash_set.iter().any(|hs| h.eq_ignore_ascii_case(hs)))
                .unwrap_or(false)
                || hash_set.contains(&d.id.to_ascii_lowercase())
        });
    }

    // Filter by category
    if let Some(ref cat) = query.category {
        if cat.is_empty() {
            torrents.retain(|d| d.category.is_none());
        } else {
            torrents.retain(|d| d.category.as_deref() == Some(cat.as_str()));
        }
    }

    // Filter by state
    if let Some(ref filter) = query.filter {
        torrents.retain(|d| match filter.as_str() {
            "downloading" => d.status == DownloadStatus::Downloading,
            "completed" => {
                d.status == DownloadStatus::Completed || d.status == DownloadStatus::Seeding
            }
            "paused" => d.status == DownloadStatus::Paused,
            "active" => {
                d.status == DownloadStatus::Downloading || d.status == DownloadStatus::Seeding
            }
            "stalled" => d.status == DownloadStatus::Queued,
            "errored" => d.status == DownloadStatus::Failed,
            _ => true,
        });
    }

    // Apply pagination (InfoQuery exposes limit/offset; cap to avoid huge responses).
    let offset = query.offset.unwrap_or(0);
    let limit = query.limit.unwrap_or(500).min(1000);
    let page: Vec<&Download> = torrents.iter().copied().skip(offset).take(limit).collect();
    let result: Vec<serde_json::Value> = page.iter().map(|d| to_qbit_torrent(d)).collect();
    Json(result)
}

#[derive(Deserialize)]
pub struct HashQuery {
    pub hash: Option<String>,
}

pub async fn properties(
    State(state): State<QbitState>,
    Query(query): Query<HashQuery>,
) -> impl IntoResponse {
    let hash = match query.hash {
        Some(h) => h,
        None => return (StatusCode::NOT_FOUND, "").into_response(),
    };

    let all = state.manager.get_all().await;
    let download = all.iter().find(|d| hash_matches(d, &hash));

    match download {
        Some(d) => {
            let now = chrono::Utc::now();
            let elapsed = (now - d.created_at).num_seconds();
            let is_finished =
                d.status == DownloadStatus::Seeding || d.status == DownloadStatus::Completed;
            let seeding_time = if is_finished {
                d.completed_at.map(|c| (now - c).num_seconds()).unwrap_or(0)
            } else {
                0
            };
            let progress = if is_finished || d.progress >= 100.0 {
                1.0
            } else {
                d.progress / 100.0
            };
            let amount_left = if is_finished || progress >= 1.0 {
                0
            } else {
                d.total_size.saturating_sub(d.downloaded_size)
            };
            let completed = if is_finished || progress >= 1.0 {
                d.total_size
            } else {
                d.downloaded_size
            };
            Json(serde_json::json!({
                "hash": d.info_hash.as_deref().unwrap_or(&d.id),
                "name": d.filename,
                "save_path": format!("{}/", std::path::Path::new(&d.save_path).parent().filter(|p| !p.as_os_str().is_empty()).map(|p| p.to_string_lossy()).unwrap_or(std::borrow::Cow::Borrowed(&d.save_path))),
                "creation_date": d.created_at.timestamp(),
                "addition_date": d.created_at.timestamp(),
                "completion_date": d.completed_at.map(|c| c.timestamp()).unwrap_or(-1),
                "piece_size": 262144,
                "pieces_num": if d.total_size > 0 { (d.total_size / 262144).max(1) } else { 0 },
                "pieces_have": if d.total_size > 0 { ((d.downloaded_size as f64 / d.total_size as f64) * (d.total_size / 262144).max(1) as f64) as u64 } else { 0 },
                "dl_speed": d.speed,
                "dl_speed_avg": d.speed,
                "up_speed": d.upload_speed,
                "up_speed_avg": d.upload_speed,
                "eta": d.eta.as_ref().map(|e| parse_eta_to_secs(e)).unwrap_or(8640000),
                "total_downloaded": d.downloaded_size,
                "total_uploaded": 0,
                "total_size": d.total_size,
                "nb_connections": d.connections,
                "nb_connections_limit": 100,
                "peers": d.peers,
                "peers_total": d.peers,
                "seeds": d.seeds,
                "seeds_total": d.seeds,
                "share_ratio": 0,
                "time_elapsed": elapsed,
                "seeding_time": seeding_time,
                "amount_left": amount_left,
                "completed": completed,
                "dl_limit": -1,
                "up_limit": -1,
                "comment": "",
                "created_by": "dload",
                "total_wasted": 0,
                "reannounce": 0,
                "last_seen": now.timestamp(),
            }))
            .into_response()
        }
        None => (StatusCode::NOT_FOUND, "").into_response(),
    }
}

pub async fn files(
    State(state): State<QbitState>,
    Query(query): Query<HashQuery>,
) -> impl IntoResponse {
    let hash = match query.hash {
        Some(h) => h,
        None => return Json(serde_json::json!([])).into_response(),
    };

    let all = state.manager.get_all().await;
    let download = all.iter().find(|d| hash_matches(d, &hash));

    match download {
        Some(d) => {
            let save_path = d.save_path.clone();
            let filename = d.filename.clone();
            let total_size = d.total_size;
            let progress = if d.progress >= 100.0 {
                1.0
            } else {
                d.progress / 100.0
            };
            let is_seed =
                d.status == DownloadStatus::Seeding || d.status == DownloadStatus::Completed;

            // Run blocking filesystem I/O off the async runtime
            let file_list = tokio::task::spawn_blocking(move || {
                let path = std::path::Path::new(&save_path);
                let mut file_list = Vec::new();
                if path.is_dir() {
                    let mut index = 0;
                    collect_files_recursive(
                        path,
                        path,
                        progress,
                        is_seed,
                        &mut file_list,
                        &mut index,
                    );
                } else if path.exists() {
                    let size = std::fs::metadata(path)
                        .map(|m| m.len())
                        .unwrap_or(total_size);
                    file_list.push(serde_json::json!({
                        "index": 0,
                        "name": filename,
                        "size": size,
                        "progress": progress,
                        "priority": 1,
                        "is_seed": is_seed,
                    }));
                }
                file_list
            })
            .await
            .unwrap_or_default();

            Json(serde_json::json!(file_list)).into_response()
        }
        None => Json(serde_json::json!([])).into_response(),
    }
}

/// Recursively collect files from a directory, building relative paths from `base`.
fn collect_files_recursive(
    base: &std::path::Path,
    dir: &std::path::Path,
    progress: f64,
    is_seed: bool,
    file_list: &mut Vec<serde_json::Value>,
    index: &mut usize,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(ft) = entry.file_type() else { continue };
        let path = entry.path();
        if ft.is_dir() {
            collect_files_recursive(base, &path, progress, is_seed, file_list, index);
        } else if ft.is_file() {
            let rel = path.strip_prefix(base).unwrap_or(&path);
            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            file_list.push(serde_json::json!({
                "index": *index,
                "name": rel.to_string_lossy().replace('\\', "/"),
                "size": size,
                "progress": progress,
                "priority": 1,
                "is_seed": is_seed,
            }));
            *index += 1;
        }
    }
}

pub async fn trackers(Query(_query): Query<HashQuery>) -> impl IntoResponse {
    Json(serde_json::json!([]))
}

pub async fn categories(State(state): State<QbitState>) -> impl IntoResponse {
    let all = state.manager.get_all().await;
    let settings = state.manager.settings.read().await;
    let registered = state.categories.read().await;
    let mut cats = std::collections::HashMap::new();

    // Include categories registered via createCategory
    for (cat, folder_id) in registered.iter() {
        let save_path = folder_id
            .as_ref()
            .and_then(|id| settings.folder_path_by_id(id))
            .map(|p| format!("{}/", p.trim_end_matches('/')))
            .unwrap_or_default();
        cats.entry(cat.clone()).or_insert_with(|| {
            serde_json::json!({
                "name": cat,
                "savePath": save_path,
            })
        });
    }

    // Include categories from existing downloads
    for d in &all {
        if let Some(ref cat) = d.category {
            cats.entry(cat.clone()).or_insert_with(|| {
                serde_json::json!({
                    "name": cat,
                    "savePath": "",
                })
            });
        }
    }
    Json(serde_json::json!(cats))
}

pub async fn tags() -> impl IntoResponse {
    Json(serde_json::json!([]))
}

pub async fn create_category(State(state): State<QbitState>, body: String) -> impl IntoResponse {
    let params: Vec<(String, String)> = url::form_urlencoded::parse(body.as_bytes())
        .into_owned()
        .collect();
    let cat = params
        .iter()
        .find(|(k, _)| k == "category")
        .map(|(_, v)| v.clone());
    let save_path = params
        .iter()
        .find(|(k, _)| k == "savePath")
        .map(|(_, v)| v.clone());

    if let Some(cat) = cat {
        if !cat.is_empty() {
            let folder_id = if let Some(ref sp) = save_path {
                let sp = sp.trim_end_matches('/');
                if !sp.is_empty() {
                    let settings = state.manager.settings.read().await;
                    settings
                        .download_folders
                        .iter()
                        .find(|f| f.path.trim_end_matches('/') == sp)
                        .map(|f| f.id.clone())
                } else {
                    None
                }
            } else {
                None
            };
            state.categories.write().await.insert(cat, folder_id);
        }
    }
    (StatusCode::OK, "Ok.")
}

pub async fn add(
    State(state): State<QbitState>,
    headers: axum::http::HeaderMap,
    request: axum::extract::Request,
) -> impl IntoResponse {
    let content_type = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let result = if content_type.starts_with("multipart/form-data") {
        use axum::extract::FromRequest;
        let multipart = axum_extra::extract::Multipart::from_request(request, &state)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to parse multipart: {}", e));
        match multipart {
            Ok(mp) => handle_add_multipart(state, mp).await,
            Err(e) => Err(e),
        }
    } else if content_type.starts_with("application/x-www-form-urlencoded") {
        const MAX_URLENCODED_BYTES: usize = 10 * 1024 * 1024; // 10 MB
        let bytes = axum::body::to_bytes(request.into_body(), MAX_URLENCODED_BYTES)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to read body: {}", e));
        match bytes {
            Ok(b) => handle_add_urlencoded(state, b).await,
            Err(e) => Err(e),
        }
    } else {
        Err(anyhow::anyhow!(
            "Unsupported content type: {}",
            content_type
        ))
    };

    match result {
        Ok(_) => (StatusCode::OK, "Ok.").into_response(),
        Err(e) => {
            tracing::error!("qBit compat add failed: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "Fails.").into_response()
        }
    }
}

async fn handle_add_urlencoded(state: QbitState, bytes: axum::body::Bytes) -> anyhow::Result<()> {
    let params: Vec<(String, String)> = url::form_urlencoded::parse(&bytes).into_owned().collect();

    let mut urls: Vec<String> = Vec::new();
    let mut category: Option<String> = None;
    let mut savepath: Option<String> = None;

    for (k, v) in params {
        match k.as_str() {
            "urls" => {
                for line in v.lines() {
                    let line = line.trim();
                    if !line.is_empty() {
                        urls.push(line.to_string());
                    }
                }
            }
            "category" if !v.is_empty() => {
                category = Some(v);
            }
            "savepath" if !v.is_empty() => {
                let p = v.trim();
                if p.starts_with('/') && !p.contains("..") {
                    savepath = Some(p.to_string());
                } else {
                    tracing::warn!("qbit_compat: rejected invalid savepath: {}", v);
                }
            }
            _ => {}
        }
    }

    let download_dir =
        resolve_qbit_download_dir(&state, category.as_deref(), savepath.as_deref()).await;

    for url in urls {
        let mut download = Download::new(url.clone(), &download_dir);
        download.protocol = Protocol::Torrent;
        download.info_hash = crate::worker::extract_info_hash(&url);
        download.category = category.clone();
        download.content_path = Some(download.save_path.clone());
        state.manager.add_and_maybe_start(download).await;
    }

    Ok(())
}

async fn handle_add_multipart(
    state: QbitState,
    mut multipart: axum_extra::extract::Multipart,
) -> anyhow::Result<()> {
    let mut urls: Vec<String> = Vec::new();
    let mut torrent_bytes: Vec<Vec<u8>> = Vec::new();
    let mut category: Option<String> = None;
    let mut savepath: Option<String> = None;
    let mut total_torrent_bytes: usize = 0;

    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "urls" => {
                let text = field.text().await?;
                for line in text.lines() {
                    let line = line.trim();
                    if !line.is_empty() {
                        urls.push(line.to_string());
                    }
                }
            }
            "torrents" => {
                const MAX_TORRENT_BYTES: usize = 10 * 1024 * 1024; // 10 MB
                const MAX_TOTAL_TORRENT_BYTES: usize = 100 * 1024 * 1024; // 100 MB aggregate
                let bytes = field.bytes().await?;
                if bytes.len() > MAX_TORRENT_BYTES {
                    anyhow::bail!("Torrent file too large");
                }
                total_torrent_bytes = total_torrent_bytes.saturating_add(bytes.len());
                if total_torrent_bytes > MAX_TOTAL_TORRENT_BYTES {
                    anyhow::bail!("Aggregate torrent payload too large");
                }
                if !bytes.is_empty() {
                    torrent_bytes.push(bytes.to_vec());
                }
            }
            "category" => {
                let text = field.text().await?;
                if !text.is_empty() {
                    category = Some(text);
                }
            }
            "savepath" => {
                let text = field.text().await?;
                if !text.is_empty() {
                    let p = text.trim();
                    if p.starts_with('/') && !p.contains("..") {
                        savepath = Some(p.to_string());
                    } else {
                        tracing::warn!("qbit_compat: rejected invalid savepath: {}", text);
                    }
                }
            }
            _ => {
                // Consume unknown fields (stopped, tags, dlLimit, etc.) to advance the stream
                if let Err(e) = field.bytes().await {
                    tracing::warn!("Failed to consume multipart field '{}': {}", name, e);
                }
            }
        }
    }

    let download_dir =
        resolve_qbit_download_dir(&state, category.as_deref(), savepath.as_deref()).await;

    // Process magnet links / URLs
    for url in urls {
        let mut download = Download::new(url.clone(), &download_dir);
        download.protocol = Protocol::Torrent;
        download.info_hash = crate::worker::extract_info_hash(&url);
        download.category = category.clone();
        download.content_path = Some(download.save_path.clone());
        state.manager.add_and_maybe_start(download).await;
    }

    // Process uploaded .torrent files via the shared manager helper so we get
    // atomic queue/start semantics and consistent persistence behavior. The
    // helper logs and returns None for any blob it can't parse; we continue so
    // Sonarr/Radarr don't see a batch failure if one of several torrents is bad.
    for bytes in torrent_bytes {
        let _ = state
            .manager
            .add_torrent_from_bytes(bytes, &download_dir, category.clone())
            .await;
    }

    Ok(())
}

pub async fn delete(State(state): State<QbitState>, body: String) -> impl IntoResponse {
    let params: Vec<(String, String)> = url::form_urlencoded::parse(body.as_bytes())
        .into_owned()
        .collect();

    let hashes = params
        .iter()
        .find(|(k, _)| k == "hashes")
        .map(|(_, v)| v.as_str())
        .unwrap_or("");
    let delete_files = params
        .iter()
        .find(|(k, _)| k == "deleteFiles")
        .map(|(_, v)| v.eq_ignore_ascii_case("true") || v == "1")
        .unwrap_or(false);

    let all = state.manager.get_all().await;

    for hash in hashes.split('|') {
        let hash = hash.trim();
        if hash.is_empty() {
            continue;
        }

        if hash == "all" {
            for d in &all {
                if d.protocol == Protocol::Torrent {
                    if delete_files {
                        state.manager.remove_with_files(&d.id).await;
                    } else {
                        state.manager.remove(&d.id).await;
                    }
                }
            }
            break;
        }

        if let Some(d) = all.iter().find(|d| hash_matches(d, hash)) {
            if delete_files {
                state.manager.remove_with_files(&d.id).await;
            } else {
                state.manager.remove(&d.id).await;
            }
        }
    }

    (StatusCode::OK, "Ok.")
}

pub async fn pause(State(state): State<QbitState>, body: String) -> impl IntoResponse {
    let params: Vec<(String, String)> = url::form_urlencoded::parse(body.as_bytes())
        .into_owned()
        .collect();
    let hashes = params
        .iter()
        .find(|(k, _)| k == "hashes")
        .map(|(_, v)| v.as_str())
        .unwrap_or("");

    let all = state.manager.get_all().await;

    for hash in hashes.split('|') {
        let hash = hash.trim();
        if hash.is_empty() {
            continue;
        }
        if hash == "all" {
            for d in &all {
                if d.protocol == Protocol::Torrent {
                    state.manager.pause_download(&d.id).await;
                }
            }
            break;
        }
        if let Some(d) = all.iter().find(|d| hash_matches(d, hash)) {
            state.manager.pause_download(&d.id).await;
        }
    }

    (StatusCode::OK, "Ok.")
}

pub async fn resume(State(state): State<QbitState>, body: String) -> impl IntoResponse {
    let params: Vec<(String, String)> = url::form_urlencoded::parse(body.as_bytes())
        .into_owned()
        .collect();
    let hashes = params
        .iter()
        .find(|(k, _)| k == "hashes")
        .map(|(_, v)| v.as_str())
        .unwrap_or("");

    let all = state.manager.get_all().await;

    for hash in hashes.split('|') {
        let hash = hash.trim();
        if hash.is_empty() {
            continue;
        }
        if hash == "all" {
            let ids: Vec<String> = all
                .iter()
                .filter(|d| d.protocol == Protocol::Torrent)
                .map(|d| d.id.clone())
                .collect();
            let mgr = state.manager.clone();
            tokio::spawn(async move {
                mgr.resume_all_downloads(ids, 2).await;
            });
            break;
        }
        if let Some(d) = all.iter().find(|d| hash_matches(d, hash)) {
            state.manager.resume_download(&d.id).await;
        }
    }

    (StatusCode::OK, "Ok.")
}

pub async fn set_category(State(state): State<QbitState>, body: String) -> impl IntoResponse {
    let params: Vec<(String, String)> = url::form_urlencoded::parse(body.as_bytes())
        .into_owned()
        .collect();
    let hashes = params
        .iter()
        .find(|(k, _)| k == "hashes")
        .map(|(_, v)| v.as_str())
        .unwrap_or("");
    let category = params
        .iter()
        .find(|(k, _)| k == "category")
        .map(|(_, v)| v.clone());

    let all = state.manager.get_all().await;

    for hash in hashes.split('|') {
        let hash = hash.trim();
        if hash.is_empty() {
            continue;
        }
        if let Some(d) = all.iter().find(|d| hash_matches(d, hash)) {
            let mut updated = d.clone();
            updated.category = category.clone();
            state.manager.update_download(&updated).await;
        }
    }

    (StatusCode::OK, "Ok.")
}

pub async fn remove_completed(State(state): State<QbitState>, body: String) -> impl IntoResponse {
    let params: Vec<(String, String)> = url::form_urlencoded::parse(body.as_bytes())
        .into_owned()
        .collect();
    let delete_files = params
        .iter()
        .find(|(k, _)| k == "deleteFiles")
        .map(|(_, v)| v.eq_ignore_ascii_case("true") || v == "1")
        .unwrap_or(false);

    let all = state.manager.get_all().await;

    for d in &all {
        if d.protocol == Protocol::Torrent
            && (d.status == DownloadStatus::Completed || d.status == DownloadStatus::Seeding)
        {
            if delete_files {
                state.manager.remove_with_files(&d.id).await;
            } else {
                state.manager.remove(&d.id).await;
            }
        }
    }

    (StatusCode::OK, "Ok.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::DownloadStatus;

    #[test]
    fn qbit_state_completed_enables_can_move_files() {
        assert_eq!(to_qbit_state(&DownloadStatus::Completed), "pausedUP");
    }

    #[test]
    fn qbit_state_seeding_enables_can_move_files() {
        assert_eq!(to_qbit_state(&DownloadStatus::Seeding), "pausedUP");
    }

    #[test]
    fn parse_eta_known_units() {
        assert_eq!(parse_eta_to_secs("1h30m15s"), 5415);
        assert_eq!(parse_eta_to_secs("2h"), 7200);
        assert_eq!(parse_eta_to_secs("45m"), 2700);
        assert_eq!(parse_eta_to_secs("90"), 90);
    }

    #[test]
    fn to_qbit_torrent_save_path_ends_with_slash() {
        let d = Download::new("magnet:?xt=urn:btih:ABC".into(), "/media/tv");
        let json = to_qbit_torrent(&d);
        let sp = json["save_path"].as_str().unwrap();
        assert!(sp.ends_with('/'), "save_path should end with /: {sp}");
        assert!(
            sp.starts_with("/media/tv"),
            "save_path should be parent dir"
        );
    }

    #[test]
    fn to_qbit_torrent_preserves_category() {
        let mut d = Download::new("magnet:?xt=urn:btih:ABC".into(), "/media/tv");
        d.category = Some("tv-sonarr".into());
        d.info_hash = Some("abc123".into());
        d.protocol = crate::domain::Protocol::Torrent;
        let json = to_qbit_torrent(&d);
        assert_eq!(json["category"].as_str().unwrap(), "tv-sonarr");
    }

    #[test]
    fn qbit_state_all_variants_are_valid() {
        let valid = [
            "downloading",
            "stalledDL",
            "queuedDL",
            "pausedDL",
            "stoppedDL",
            "checkingDL",
            "forcedDL",
            "metaDL",
            "allocating",
            "moving",
            "uploading",
            "stalledUP",
            "queuedUP",
            "pausedUP",
            "stoppedUP",
            "checkingUP",
            "forcedUP",
            "error",
            "missingFiles",
            "unknown",
        ];
        let all = [
            DownloadStatus::Queued,
            DownloadStatus::Downloading,
            DownloadStatus::Paused,
            DownloadStatus::Completed,
            DownloadStatus::Failed,
            DownloadStatus::Stopped,
            DownloadStatus::Seeding,
        ];
        for status in &all {
            let state = to_qbit_state(status);
            assert!(
                valid.contains(&state),
                "Invalid qBit state '{}' for {:?}",
                state,
                status
            );
        }
    }
}
