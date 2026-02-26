use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use std::sync::Arc;

use super::QbitState;
use crate::domain::{Download, DownloadStatus, Protocol};

/// No-op handler for endpoints we accept but don't act on (setShareLimits, topPrio, etc.)
pub async fn noop(_body: String) -> impl IntoResponse {
    (StatusCode::OK, "Ok.")
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

/// Map dload DownloadStatus to qBittorrent v5 state string.
/// Sonarr only sets CanMoveFiles (auto-import) when state is "pausedUP" or "stoppedUP".
/// Since we claim v5.0.0, use "stopped*" variants.
fn to_qbit_state(status: &DownloadStatus) -> &'static str {
    match status {
        DownloadStatus::Queued => "queuedDL",
        DownloadStatus::Downloading => "downloading",
        DownloadStatus::Paused => "stoppedDL",
        DownloadStatus::Completed => "stoppedUP",
        DownloadStatus::Failed => "error",
        DownloadStatus::Stopped => "stoppedDL",
        DownloadStatus::Seeding => "stoppedUP",
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
    let save_path = format!("{}/", std::path::Path::new(&d.save_path)
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| p.to_string_lossy())
        .unwrap_or(std::borrow::Cow::Borrowed(&d.save_path)));

    let now = chrono::Utc::now();
    let time_active = (now - d.created_at).num_seconds();
    let seeding_time = if d.status == DownloadStatus::Seeding || d.status == DownloadStatus::Completed {
        d.completed_at.map(|c| (now - c).num_seconds()).unwrap_or(0)
    } else {
        0
    };

    let progress = if d.progress >= 100.0 { 1.0 } else { d.progress / 100.0 };
    let magnet_uri = if d.url.starts_with("magnet:") { d.url.as_str() } else { "" };

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
        "amount_left": d.total_size.saturating_sub(d.downloaded_size),
        "completed": d.downloaded_size,
        "total_size": d.total_size,
        "time_active": time_active,
        "tracker": "",
        "magnet_uri": magnet_uri,
        "priority": 0,
        "seq_dl": false,
        "f_l_piece_prio": false,
        "auto_tmm": false,
        "availability": if d.progress >= 100.0 { -1.0 } else { progress },
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

    let result: Vec<serde_json::Value> = torrents.iter().map(|d| to_qbit_torrent(d)).collect();
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
    let download = all
        .iter()
        .find(|d| hash_matches(d, &hash));

    match download {
        Some(d) => {
            let now = chrono::Utc::now();
            let elapsed = (now - d.created_at).num_seconds();
            let seeding_time = if d.status == DownloadStatus::Seeding || d.status == DownloadStatus::Completed {
                d.completed_at.map(|c| (now - c).num_seconds()).unwrap_or(0)
            } else {
                0
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
    let download = all
        .iter()
        .find(|d| hash_matches(d, &hash));

    match download {
        Some(d) => {
            let save_path = d.save_path.clone();
            let filename = d.filename.clone();
            let total_size = d.total_size;
            let progress = if d.progress >= 100.0 { 1.0 } else { d.progress / 100.0 };
            let is_seed = d.status == DownloadStatus::Seeding || d.status == DownloadStatus::Completed;

            // Run blocking filesystem I/O off the async runtime
            let file_list = tokio::task::spawn_blocking(move || {
                let path = std::path::Path::new(&save_path);
                let mut file_list = Vec::new();
                if path.is_dir() {
                    let mut index = 0;
                    collect_files_recursive(path, path, progress, is_seed, &mut file_list, &mut index);
                } else if path.exists() {
                    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(total_size);
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
    let Ok(entries) = std::fs::read_dir(dir) else { return };
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
    let mut cats = std::collections::HashMap::new();

    // Include categories registered via createCategory
    for cat in state.categories.read().await.iter() {
        cats.entry(cat.clone())
            .or_insert_with(|| {
                serde_json::json!({
                    "name": cat,
                    "savePath": "",
                })
            });
    }

    // Include categories from existing downloads
    for d in &all {
        if let Some(ref cat) = d.category {
            cats.entry(cat.clone())
                .or_insert_with(|| {
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
    if let Some(cat) = params.iter().find(|(k, _)| k == "category").map(|(_, v)| v.clone()) {
        if !cat.is_empty() {
            state.categories.write().await.insert(cat);
        }
    }
    (StatusCode::OK, "Ok.")
}

pub async fn add(
    State(state): State<QbitState>,
    multipart: axum_extra::extract::Multipart,
) -> impl IntoResponse {
    match handle_add(state, multipart).await {
        Ok(_) => (StatusCode::OK, "Ok.").into_response(),
        Err(e) => {
            tracing::error!("qBit compat add failed: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "Fails.").into_response()
        }
    }
}

async fn handle_add(
    state: QbitState,
    mut multipart: axum_extra::extract::Multipart,
) -> anyhow::Result<()> {
    let mut urls: Vec<String> = Vec::new();
    let mut torrent_bytes: Vec<Vec<u8>> = Vec::new();
    let mut category: Option<String> = None;
    let mut savepath: Option<String> = None;

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
                let bytes = field.bytes().await?;
                if bytes.len() > MAX_TORRENT_BYTES {
                    anyhow::bail!("Torrent file too large");
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
                    savepath = Some(text);
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

    let settings = state.manager.settings.read().await;
    let download_dir = savepath.unwrap_or_else(|| settings.download_dir.clone());
    drop(settings);

    // Process magnet links / URLs
    for url in urls {
        let mut download = Download::new(url.clone(), &download_dir);
        download.protocol = Protocol::Torrent;
        download.info_hash = crate::worker::extract_info_hash(&url);
        download.category = category.clone();
        download.content_path = Some(download.save_path.clone());
        download.status = DownloadStatus::Downloading;

        state.manager.add_download(download.clone()).await;
        let mgr = Arc::clone(&state.manager);
        mgr.start_download(download).await;
    }

    // Process uploaded .torrent files
    for bytes in torrent_bytes {
        let name = librqbit::torrent_from_bytes::<librqbit::ByteBufOwned>(&bytes)
            .ok()
            .and_then(|meta| meta.info.name.as_ref().map(|n| n.to_string()))
            .unwrap_or_else(|| "torrent-download".to_string());

        let mut download = Download::new(format!("torrent://{}", name), &download_dir);
        download.filename = name.clone();
        download.save_path = format!("{}/{}", download_dir, name);
        download.category = category.clone();
        download.content_path = Some(download.save_path.clone());
        download.protocol = Protocol::Torrent;
        // info_hash will be set by session_add_and_wait after librqbit parses the torrent

        state.manager.add_download(download.clone()).await;
        let mgr = Arc::clone(&state.manager);
        mgr.start_torrent_from_bytes(download, bytes).await;
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

        if let Some(d) = all
            .iter()
            .find(|d| hash_matches(d, hash))
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
        if let Some(d) = all
            .iter()
            .find(|d| hash_matches(d, hash))
        {
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
            for d in &all {
                if d.protocol == Protocol::Torrent {
                    state.manager.resume_download(&d.id).await;
                }
            }
            break;
        }
        if let Some(d) = all
            .iter()
            .find(|d| hash_matches(d, hash))
        {
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
        if let Some(d) = all
            .iter()
            .find(|d| hash_matches(d, hash))
        {
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
