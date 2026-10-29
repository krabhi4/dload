use crate::api::qbit_compat::session::SessionStore;
use crate::db::repository::Repository;
use crate::domain::{Download, DownloadStatus, Protocol, Settings};
use crate::worker::http::HttpDownloader;
use librqbit::api::TorrentIdOrHash;
use librqbit::Session;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use tokio::sync::RwLock;
// Use tokio's Instant so tests with `#[tokio::test(start_paused = true)]` can
// advance time deterministically. In non-paused (production) mode this is a
// thin wrapper over std::time::Instant with identical semantics.
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct SpeedMeter {
    samples: std::collections::VecDeque<(Instant, u64)>,
}

impl SpeedMeter {
    fn sample(&mut self, bytes: u64) -> u64 {
        let now = Instant::now();
        if self.samples.back().is_some_and(|&(_, last)| bytes < last) {
            self.samples.clear();
        }
        self.samples.push_back((now, bytes));
        if self.samples.len() > 5 {
            self.samples.pop_front();
        }
        let (start, start_bytes) = self.samples[0];
        let secs = now.duration_since(start).as_secs_f64();
        if secs > 0.0 {
            (bytes.saturating_sub(start_bytes) as f64 / secs) as u64
        } else {
            0
        }
    }
}

fn format_eta(secs: u64) -> String {
    let (hours, mins, secs) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    if hours > 0 {
        format!("{}h{}m{}s", hours, mins, secs)
    } else if mins > 0 {
        format!("{}m{}s", mins, secs)
    } else {
        format!("{}s", secs)
    }
}

/// Per-IP token-bucket-ish limiter for login endpoints. Rejects after
/// `max_attempts` in a rolling `window`. Shared across native + qbit login paths.
struct LimiterInner {
    entries: HashMap<IpAddr, (u32, Instant)>,
    last_sweep: Instant,
}

/// Short-TTL cache of accepted Basic Auth credentials, keyed by the raw base64
/// "Authorization: Basic …" value so a password change invalidates the entry.
/// Lets Sonarr-style polling skip bcrypt on every request.
pub struct BasicAuthCache {
    entries: tokio::sync::RwLock<HashMap<String, Instant>>,
    ttl: std::time::Duration,
}

impl BasicAuthCache {
    pub fn new(ttl: std::time::Duration) -> Self {
        Self {
            entries: tokio::sync::RwLock::new(HashMap::new()),
            ttl,
        }
    }

    pub async fn is_valid(&self, key: &str) -> bool {
        let now = Instant::now();
        let entries = self.entries.read().await;
        entries.get(key).is_some_and(|expires| *expires > now)
    }

    pub async fn insert(&self, key: &str) {
        let now = Instant::now();
        let mut entries = self.entries.write().await;
        if entries.len() > 1024 {
            entries.retain(|_, expires| *expires > now);
        }
        entries.insert(key.to_string(), now + self.ttl);
    }

    pub async fn invalidate_all(&self) {
        self.entries.write().await.clear();
    }
}

pub struct LoginRateLimiter {
    inner: tokio::sync::Mutex<LimiterInner>,
    max_attempts: u32,
    window: std::time::Duration,
}

impl LoginRateLimiter {
    pub fn new(max_attempts: u32, window: std::time::Duration) -> Self {
        Self {
            inner: tokio::sync::Mutex::new(LimiterInner {
                entries: HashMap::new(),
                last_sweep: Instant::now(),
            }),
            max_attempts,
            window,
        }
    }

    pub async fn is_limited(&self, ip: IpAddr) -> bool {
        let inner = self.inner.lock().await;
        inner.entries.get(&ip).is_some_and(|(count, since)| {
            Instant::now().duration_since(*since) < self.window && *count >= self.max_attempts
        })
    }

    /// Returns `true` if the attempt is allowed and consumes one slot.
    pub async fn try_consume(&self, ip: IpAddr) -> bool {
        let now = Instant::now();
        let mut inner = self.inner.lock().await;
        // Sweep on size threshold OR when ~window has elapsed since last sweep,
        // whichever comes first. Pure size-based sweep could grow unboundedly
        // under a burst of many fresh IPs.
        let should_sweep =
            inner.entries.len() > 4096 || now.duration_since(inner.last_sweep) >= self.window;
        if should_sweep {
            let window = self.window;
            inner
                .entries
                .retain(|_, (_, ts)| now.duration_since(*ts) < window);
            inner.last_sweep = now;
        }
        let entry = inner.entries.entry(ip).or_insert((0, now));
        if now.duration_since(entry.1) >= self.window {
            *entry = (0, now);
        }
        if entry.0 >= self.max_attempts {
            return false;
        }
        entry.0 += 1;
        true
    }
}

pub fn extract_client_ip(
    headers: &axum::http::HeaderMap,
    peer: Option<std::net::SocketAddr>,
) -> Option<IpAddr> {
    let trust_proxy = std::env::var("DLOAD_TRUST_PROXY_HEADERS")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    extract_client_ip_inner(headers, peer, trust_proxy)
}

fn extract_client_ip_inner(
    headers: &axum::http::HeaderMap,
    peer: Option<std::net::SocketAddr>,
    trust_proxy: bool,
) -> Option<IpAddr> {
    if trust_proxy {
        if let Some(v) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()) {
            if let Some(last) = v.split(',').next_back() {
                if let Ok(ip) = last.trim().parse::<IpAddr>() {
                    return Some(ip);
                }
            }
        }
        if let Some(v) = headers.get("x-real-ip").and_then(|v| v.to_str().ok()) {
            if let Ok(ip) = v.trim().parse::<IpAddr>() {
                return Some(ip);
            }
        }
    }
    peer.map(|p| p.ip())
}

/// Torrent session + the download_dir it was created with.
/// Session is recreated when download_dir changes.
type TorrentSession = Arc<RwLock<Option<(String, Arc<Session>)>>>;

#[derive(Clone)]
pub struct ManagerState {
    pub downloads: Arc<RwLock<HashMap<String, Download>>>,
    pub settings: Arc<RwLock<Settings>>,
    pub repo: Arc<Repository>,
    torrent_session: TorrentSession,
    cancel_tokens: Arc<RwLock<HashMap<String, CancellationToken>>>,
    /// Maps download ID -> librqbit torrent handle ID for pause/resume
    torrent_handles: Arc<RwLock<HashMap<String, usize>>>,
    /// Serializes queue-slot check-and-claim so max_concurrent cannot be
    /// exceeded by concurrent add/promotion paths.
    slot_lock: Arc<tokio::sync::Mutex<()>>,
    pub login_limiter: Arc<LoginRateLimiter>,
    pub sessions: Arc<SessionStore>,
    pub basic_auth_cache: Arc<BasicAuthCache>,
    http_runs: Arc<std::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>>,
}

impl ManagerState {
    /// Returns `(Self, auto_resume_ids)` where `auto_resume_ids` are torrent and HTTP download IDs
    /// that were actively downloading or seeding before shutdown and should be auto-resumed.
    pub fn new(settings: Settings, repo: Arc<Repository>) -> (Self, Vec<String>) {
        let mut auto_resume_ids = Vec::new();

        // Load existing downloads from DB
        let downloads = match repo.get_all_downloads() {
            Ok(dl_list) => {
                let mut map = HashMap::new();
                for mut dl in dl_list {
                    if dl.protocol == Protocol::Http {
                        dl.protocol = crate::worker::detect_protocol(&dl.url);
                    }
                    // Collect torrent and HTTP downloads with restart_resume flag for auto-resume
                    if dl.restart_resume
                        && dl.status != DownloadStatus::Queued
                        && dl.status != DownloadStatus::Failed
                        && matches!(dl.protocol, Protocol::Torrent | Protocol::Http)
                    {
                        auto_resume_ids.push(dl.id.clone());
                    }

                    // Downloads that were in-progress when the app stopped are now paused
                    if dl.status == DownloadStatus::Downloading
                        || dl.status == DownloadStatus::Fetching
                    {
                        dl.status = DownloadStatus::Paused;
                        dl.speed = 0;
                        dl.upload_speed = 0;
                        let _ = repo.update_download(&dl);
                    }
                    // Torrents that were seeding → paused (so they can auto-resume back to seeding)
                    // HTTP seeding shouldn't happen, but mark completed if it does
                    if dl.status == DownloadStatus::Seeding {
                        if dl.protocol == Protocol::Torrent {
                            dl.status = DownloadStatus::Paused;
                        } else {
                            dl.status = DownloadStatus::Completed;
                        }
                        dl.speed = 0;
                        dl.upload_speed = 0;
                        let _ = repo.update_download(&dl);
                    }
                    // Clean up interrupted mirror operations from previous run
                    if dl.http_mirror_status.is_some() {
                        dl.http_mirror_status = None;
                        dl.http_mirror_url = None;
                        let _ = repo.update_download(&dl);

                        // Clean up leftover temp files (.mirror and .zip) in save_path parent
                        let tmp_dir = std::path::Path::new(&dl.save_path)
                            .parent()
                            .map(|p| p.join(".tmp"))
                            .unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
                        for ext in &["mirror", "zip"] {
                            let tmp_file = tmp_dir.join(format!("{}.{}", dl.id, ext));
                            if tmp_file.exists() {
                                let _ = std::fs::remove_file(&tmp_file);
                            }
                        }
                    }
                    map.insert(dl.id.clone(), dl);
                }
                tracing::info!("Restored {} downloads from database", map.len());
                map
            }
            Err(e) => {
                tracing::error!("Failed to load downloads from database: {}", e);
                HashMap::new()
            }
        };

        (
            Self {
                downloads: Arc::new(RwLock::new(downloads)),
                settings: Arc::new(RwLock::new(settings)),
                repo,
                torrent_session: Arc::new(RwLock::new(None)),
                cancel_tokens: Arc::new(RwLock::new(HashMap::new())),
                torrent_handles: Arc::new(RwLock::new(HashMap::new())),
                slot_lock: Arc::new(tokio::sync::Mutex::new(())),
                login_limiter: Arc::new(LoginRateLimiter::new(
                    10,
                    std::time::Duration::from_secs(60),
                )),
                sessions: Arc::new(SessionStore::new()),
                basic_auth_cache: Arc::new(BasicAuthCache::new(std::time::Duration::from_secs(
                    300,
                ))),
                http_runs: Arc::new(std::sync::Mutex::new(HashMap::new())),
            },
            auto_resume_ids,
        )
    }

    /// Read the current download directory from settings (always up-to-date).
    pub async fn download_dir(&self) -> String {
        self.settings.read().await.download_dir.clone()
    }

    /// Run a blocking repo operation off the async runtime.
    pub async fn repo_blocking<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&crate::db::repository::Repository) -> R + Send + 'static,
        R: Send + 'static,
    {
        let repo = Arc::clone(&self.repo);
        tokio::task::spawn_blocking(move || f(&repo))
            .await
            .expect("repo task panicked")
    }

    /// Authenticate a login JWT or an API key (by the `dload_` prefix) — the
    /// download/service surface accepts both.
    pub async fn authenticate(&self, token: &str) -> Result<crate::domain::User, &'static str> {
        if token.starts_with(crate::domain::API_KEY_PREFIX) {
            self.authenticate_api_key(token).await
        } else {
            self.authenticate_jwt(token).await
        }
    }

    /// Decode a JWT, then confirm its subject still exists in the DB and return
    /// that user (DB role authoritative). The secret is stable, so signature +
    /// expiry alone keeps accepting tokens after the user is deleted or the DB is
    /// wiped — re-checking the DB closes that hole.
    async fn authenticate_jwt(&self, token: &str) -> Result<crate::domain::User, &'static str> {
        let validation = jsonwebtoken::Validation {
            algorithms: vec![jsonwebtoken::Algorithm::HS256],
            ..Default::default()
        };
        let claims = jsonwebtoken::decode::<crate::domain::Claims>(
            token,
            &jsonwebtoken::DecodingKey::from_secret(crate::domain::jwt_secret()),
            &validation,
        )
        .map(|d| d.claims)
        .map_err(|_| "Authentication required")?;

        let sub = claims.sub;
        let token_ver = claims.ver;
        let user = self
            .repo_blocking(move |repo| repo.get_user_by_username(&sub))
            .await
            .ok()
            .flatten()
            .ok_or("Authentication required")?;

        if user.token_version != token_ver {
            return Err("Authentication required");
        }
        Ok(user)
    }

    async fn authenticate_api_key(&self, key: &str) -> Result<crate::domain::User, &'static str> {
        let hash = crate::domain::hash_api_key(key);
        let hash_lookup = hash.clone();
        let user = self
            .repo_blocking(move |repo| repo.get_user_by_api_key_hash(&hash_lookup))
            .await
            .ok()
            .flatten()
            .ok_or("Authentication required")?;

        let now = chrono::Utc::now();
        let now_s = now.to_rfc3339_opts(chrono::SecondsFormat::Micros, true);
        let cutoff_s = (now - chrono::Duration::seconds(60))
            .to_rfc3339_opts(chrono::SecondsFormat::Micros, true);
        let _ = self
            .repo_blocking(move |repo| repo.touch_api_key(&hash, &now_s, &cutoff_s))
            .await;

        Ok(user)
    }

    /// JWT-only authentication — rejects API keys. Used by account-security and
    /// admin-management endpoints so a leaked API key cannot reach them.
    pub async fn authenticate_session(
        &self,
        token: &str,
    ) -> Result<crate::domain::User, &'static str> {
        if token.starts_with(crate::domain::API_KEY_PREFIX) {
            return Err("Authentication required");
        }
        self.authenticate_jwt(token).await
    }

    /// Like [`authenticate`](Self::authenticate) but requires the live DB user
    /// to be an admin (DB role, not the token's — a demoted admin's token fails).
    pub async fn authenticate_admin(
        &self,
        token: &str,
    ) -> Result<crate::domain::User, &'static str> {
        match self.authenticate(token).await {
            Ok(u) if u.role == crate::domain::Role::Admin => Ok(u),
            Ok(_) => Err("Admin access required"),
            Err(e) => Err(e),
        }
    }

    /// JWT-only admin check (rejects API keys) for admin-management endpoints.
    pub async fn authenticate_session_admin(
        &self,
        token: &str,
    ) -> Result<crate::domain::User, &'static str> {
        match self.authenticate_session(token).await {
            Ok(u) if u.role == crate::domain::Role::Admin => Ok(u),
            Ok(_) => Err("Admin access required"),
            Err(e) => Err(e),
        }
    }

    async fn get_torrent_session(&self) -> anyhow::Result<Arc<Session>> {
        // Hold write lock for the entire check-and-create to prevent races
        let mut guard = self.torrent_session.write().await;
        let dir = self.download_dir().await;

        // Every add passes an explicit output folder, so one session serves all download
        // folders. Recreating it would leave the old one holding the BitTorrent port.
        if let Some((_, ref session)) = *guard {
            return Ok(session.clone());
        }

        // Ensure the directory exists before initializing librqbit so DHT persistence succeeds
        if let Err(e) = tokio::fs::create_dir_all(&dir).await {
            tracing::warn!("Failed to create download directory {}: {}", dir, e);
        }

        // BitTorrent listen port for INCOMING peer connections. Fixed to a single port (not a
        // range) and configurable via DLOAD_BT_PORT so it can be matched to a published/
        // forwarded Docker port. Without a reachable inbound port no peer can connect to us,
        // so the upload/seeding ratio stays pinned at 0. Default 6881 (the BitTorrent norm).
        let bt_port: u16 = std::env::var("DLOAD_BT_PORT")
            .ok()
            .and_then(|v| v.parse::<u16>().ok())
            .filter(|p| *p >= 1 && *p < 65535)
            .unwrap_or(6881);

        let opts = librqbit::SessionOptions {
            listen: Some(librqbit::ListenerOptions {
                listen_addr: ([0, 0, 0, 0], bt_port).into(),
                enable_upnp_port_forwarding: true,
                ..Default::default()
            }),
            fastresume: true,
            concurrent_init_limit: Some(5),
            connect: Some(librqbit::ConnectionOptions {
                peer_opts: Some(librqbit::PeerConnectionOptions {
                    connect_timeout: Some(std::time::Duration::from_secs(5)),
                    read_write_timeout: Some(std::time::Duration::from_secs(10)),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            trackers: [
                "udp://tracker.opentrackr.org:1337/announce",
                "udp://open.stealth.si:80/announce",
                "udp://tracker.torrent.eu.org:451/announce",
                "udp://open.demonii.com:1337/announce",
                "udp://explodie.org:6969/announce",
                "udp://tracker.tiny-vps.com:6969/announce",
                "udp://tracker.openbittorrent.com:6969/announce",
                "udp://tracker.pomf.se:80/announce",
                "udp://p4p.arenabg.com:1337/announce",
                "udp://exodus.desync.com:6969/announce",
            ]
            .iter()
            .filter_map(|u| url::Url::parse(u).ok())
            .collect(),
            ..Default::default()
        };

        let session = Session::new_with_opts(dir.clone().into(), opts)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to create torrent session: {}", e))?;

        *guard = Some((dir, session.clone()));
        Ok(session)
    }

    pub async fn add_download(&self, download: Download) {
        let dl = download.clone();
        if let Err(e) = self
            .repo_blocking(move |repo| repo.insert_download(&dl))
            .await
        {
            tracing::error!("Failed to persist download to DB: {}", e);
        }
        // Record in history
        let dl2 = download.clone();
        if let Err(e) = self
            .repo_blocking(move |repo| repo.insert_history(&dl2))
            .await
        {
            tracing::error!("Failed to record download in history: {}", e);
        }
        let mut downloads = self.downloads.write().await;
        downloads.insert(download.id.clone(), download);
    }

    /// Count downloads that are actively running (Downloading or Seeding).
    pub async fn has_torrent(&self, info_hash: &str) -> bool {
        self.downloads.read().await.values().any(|d| {
            d.status != DownloadStatus::Failed
                && d.info_hash
                    .as_deref()
                    .is_some_and(|h| h.eq_ignore_ascii_case(info_hash))
        })
    }

    pub async fn active_download_count(&self) -> usize {
        let downloads = self.downloads.read().await;
        downloads
            .values()
            .filter(|d| {
                d.status == DownloadStatus::Downloading || d.status == DownloadStatus::Fetching
            })
            .count()
    }

    /// Assign a position for a new download (after all existing non-completed downloads).
    async fn assign_new_download_position(&self) -> i32 {
        let downloads = self.downloads.read().await;
        downloads
            .values()
            .filter(|d| d.status != DownloadStatus::Completed)
            .map(|d| d.position)
            .max()
            .map(|m| m + 1)
            .unwrap_or(0)
    }

    /// Slot claim without starting a worker. Only used by concurrency tests
    /// to exercise the atomic check-and-claim under `slot_lock` without the
    /// real network / librqbit side effects that make `add_and_maybe_start`
    /// awkward to test. Kept `pub` so integration tests under `tests/` can
    /// see it; not part of the public API.
    #[doc(hidden)]
    #[allow(dead_code)] // reachable only from integration tests
    pub async fn claim_or_queue_for_test(
        self: &Arc<Self>,
        mut download: Download,
    ) -> DownloadStatus {
        download.position = self.assign_new_download_position().await;
        let _slot_guard = self.slot_lock.lock().await;
        let max_concurrent = self.settings.read().await.max_concurrent as usize;
        let active = self.active_download_count().await;
        let status = if active >= max_concurrent {
            DownloadStatus::Queued
        } else {
            DownloadStatus::Downloading
        };
        download.status = status.clone();
        self.add_download(download).await;
        status
    }

    /// Add a download and start it if under the concurrent limit, otherwise queue it.
    pub async fn add_and_maybe_start(self: &Arc<Self>, mut download: Download) {
        download.position = self.assign_new_download_position().await;
        // Serialize with try_start_queued so check-and-claim is atomic.
        let _slot_guard = self.slot_lock.lock().await;
        let max_concurrent = self.settings.read().await.max_concurrent as usize;
        let active = self.active_download_count().await;

        if active >= max_concurrent {
            download.status = DownloadStatus::Queued;
            if download.protocol == Protocol::Torrent {
                download.restart_resume = true;
            }
            self.add_download(download).await;
        } else {
            download.status = DownloadStatus::Downloading;
            self.add_download(download.clone()).await;
            // Slot now claimed (download is in active_download_count). Drop lock
            // before spawning the worker so concurrent paths don't wait on I/O.
            drop(_slot_guard);
            let state = Arc::clone(self);
            state.start_download(download).await;
        }
    }

    /// Materialize a `Download` from raw `.torrent` bytes and either start it
    /// immediately or queue it, persisting the bytes under `<data_dir>/torrents`
    /// so queued downloads can be resumed after restart. Used by both the native
    /// and qBittorrent-compat multipart endpoints. Returns `None` if the bytes
    /// are not a valid `.torrent` file.
    pub async fn add_torrent_from_bytes(
        self: &Arc<Self>,
        bytes: Vec<u8>,
        download_dir: &str,
        category: Option<String>,
    ) -> Option<Download> {
        let meta = match librqbit::torrent_from_bytes(&bytes) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!("rejecting invalid .torrent upload: {}", e);
                return None;
            }
        };
        if self.has_torrent(&meta.info_hash.as_string()).await {
            tracing::info!("skipping .torrent upload already in downloads");
            return None;
        }
        let raw_name = meta
            .info
            .data
            .name
            .as_ref()
            .map(|n| n.to_string())
            .unwrap_or_else(|| "torrent-download".to_string());
        let name = crate::domain::sanitize_filename(&raw_name);

        let mut download = Download::new(format!("torrent://{}", name), download_dir);
        download.filename = name.clone();
        download.save_path = format!("{}/{}", download.download_folder, name);
        download.category = category;
        download.info_hash = Some(meta.info_hash.as_string());
        // Don't pre-set content_path — session_add_and_wait writes the authoritative value
        // once librqbit has the metadata. Pre-metadata status is Queued/Fetching → *arr
        // treats it as non-completed and won't try to import.
        download.content_path = None;
        download.protocol = Protocol::Torrent;
        download.position = self.assign_new_download_position().await;

        // Atomic check-and-claim: only the count-check + status flip + DB insert
        // must run under slot_lock. Filesystem persistence happens after we drop
        // the lock so concurrent submissions aren't serialized on disk I/O.
        let queued = {
            let _slot_guard = self.slot_lock.lock().await;
            let max_concurrent = self.settings.read().await.max_concurrent as usize;
            let active = self.active_download_count().await;
            if active >= max_concurrent {
                download.status = DownloadStatus::Queued;
                download.restart_resume = true;
                self.add_download(download.clone()).await;
                true
            } else {
                download.status = DownloadStatus::Downloading;
                self.add_download(download.clone()).await;
                false
            }
        };

        if queued {
            if let Err(e) = write_persisted_torrent(&download.id, &bytes).await {
                tracing::warn!("Failed to persist .torrent file: {}", e);
            }
        } else {
            Arc::clone(self)
                .start_torrent_from_bytes(download.clone(), bytes)
                .await;
        }

        Some(download)
    }

    /// Promote the oldest queued download when a slot opens.
    /// Call this after a download completes, fails, pauses, or is deleted.
    pub async fn try_start_queued(self: &Arc<Self>) {
        // Serialize with add_and_maybe_start so check-and-claim is atomic.
        let _slot_guard = self.slot_lock.lock().await;
        let max_concurrent = self.settings.read().await.max_concurrent as usize;
        let active = self.active_download_count().await;

        if active >= max_concurrent {
            return;
        }

        // Find the highest-priority queued download (by position) and claim
        // slots by flipping their status to Downloading before releasing the lock.
        let promote_ids = {
            let mut downloads = self.downloads.write().await;
            let mut queued: Vec<_> = downloads
                .values()
                .filter(|d| d.status == DownloadStatus::Queued)
                .map(|d| (d.id.clone(), d.position))
                .collect();
            queued.sort_by_key(|(_, pos)| *pos);
            let to_take = max_concurrent - active;
            let ids: Vec<String> = queued.into_iter().take(to_take).map(|(id, _)| id).collect();
            for id in &ids {
                if let Some(d) = downloads.get_mut(id) {
                    d.status = DownloadStatus::Downloading;
                }
            }
            ids
        };
        drop(_slot_guard);

        for id in promote_ids {
            tracing::info!("Promoting queued download: {}", id);
            let state = Arc::clone(self);
            // Use launch (not start_download) so displaced torrents
            // with live paused handles get properly unpaused via the fast path.
            tokio::spawn(async move {
                if let Some(d) = state.get_download(&id).await {
                    state.launch(&id, d).await;
                }
            });
        }
    }

    pub async fn update_download(&self, download: &Download) {
        let mut dl = download.clone();
        {
            let mut downloads = self.downloads.write().await;
            // Push failed downloads to the bottom of the list
            if dl.status == DownloadStatus::Failed {
                let max_pos = downloads.values().map(|d| d.position).max().unwrap_or(0);
                dl.position = max_pos + 1;
            }
            if let Some(d) = downloads.get_mut(&dl.id) {
                *d = dl.clone();
            }
        }
        if let Err(e) = self
            .repo_blocking(move |repo| repo.update_download(&dl))
            .await
        {
            tracing::error!("Failed to update download in DB: {}", e);
        }
        // Update history on terminal status changes
        match download.status {
            DownloadStatus::Completed | DownloadStatus::Failed => {
                let dl2 = download.clone();
                if let Err(e) = self
                    .repo_blocking(move |repo| repo.update_history(&dl2))
                    .await
                {
                    tracing::error!("Failed to update history: {}", e);
                }
            }
            _ => {}
        }
    }

    pub async fn get_all(&self) -> Vec<Download> {
        let downloads = self.downloads.read().await;
        downloads.values().cloned().collect()
    }

    /// Reorder downloads by assigning positions from the given ordered ID list.
    /// Queued downloads among the top `max_concurrent` active or queued positions are started;
    /// active downloads pushed below are queued. Paused, stopped and failed ones keep their status.
    pub async fn reorder_downloads(self: &Arc<Self>, ordered_ids: Vec<String>) {
        let max_concurrent = self.settings.read().await.max_concurrent as usize;

        // Build position map and collect any missing IDs not in the request
        let (position_map, to_start, to_queue) = {
            let mut downloads = self.downloads.write().await;

            // Append any downloads not in the request, maintaining their relative order
            let ordered_set: std::collections::HashSet<&String> = ordered_ids.iter().collect();
            let mut missing: Vec<_> = downloads
                .values()
                .filter(|d| !ordered_set.contains(&d.id) && d.status != DownloadStatus::Completed)
                .cloned()
                .collect();
            missing.sort_by_key(|d| d.position);

            let full_ordered: Vec<String> = ordered_ids
                .iter()
                .cloned()
                .chain(missing.iter().map(|d| d.id.clone()))
                .collect();

            let mut pos_map: Vec<(String, i32)> = Vec::new();
            let mut start_ids: Vec<String> = Vec::new();
            let mut queue_ids: Vec<String> = Vec::new();

            let mut runnable_rank = 0usize;
            for (i, id) in full_ordered.iter().enumerate() {
                if let Some(d) = downloads.get_mut(id) {
                    d.position = i as i32;
                    pos_map.push((id.clone(), i as i32));

                    match d.status {
                        DownloadStatus::Downloading | DownloadStatus::Fetching => {
                            if runnable_rank >= max_concurrent {
                                queue_ids.push(id.clone());
                            }
                            runnable_rank += 1;
                        }
                        DownloadStatus::Queued => {
                            if runnable_rank < max_concurrent {
                                start_ids.push(id.clone());
                            }
                            runnable_rank += 1;
                        }
                        DownloadStatus::Seeding => {
                            // Never demote Seeding — arr stack needs pausedUP to import/delete
                        }
                        DownloadStatus::Paused
                        | DownloadStatus::Failed
                        | DownloadStatus::Stopped
                        | DownloadStatus::Completed => {}
                    }
                }
            }

            (pos_map, start_ids, queue_ids)
        };

        // Persist positions to DB in a single transaction
        if !position_map.is_empty() {
            let pairs = position_map.clone();
            if let Err(e) = self
                .repo_blocking(move |repo| repo.update_positions(&pairs))
                .await
            {
                tracing::error!("Failed to persist positions: {}", e);
            }
        }

        // Demote active downloads that were moved below top N (do this first to free slots).
        // We avoid calling pause_download() here because it sets an intermediate Paused
        // status that can race with the monitor_torrent loop (which also detects is_paused
        // and overwrites the status). Instead, we stop the download work and set Queued
        // directly in one step.
        for id in &to_queue {
            // Stop the actual download work
            let torrent_id = {
                let handles = self.torrent_handles.read().await;
                handles.get(id).copied()
            };
            if let Some(tid) = torrent_id {
                // Torrent: use native pause (keeps librqbit state for fast unpause later)
                if let Ok(session) = self.get_torrent_session().await {
                    if let Some(handle) = session.get(TorrentIdOrHash::Id(tid)) {
                        let _ = session.pause(&handle).await;
                    }
                }
            } else {
                // HTTP: fire cancellation token to stop the worker
                self.cancel_download(id).await;
            }

            // Set status directly to Queued (skip the Paused intermediate state)
            let snap = {
                let mut downloads = self.downloads.write().await;
                if let Some(d) = downloads.get_mut(id) {
                    d.status = DownloadStatus::Queued;
                    d.speed = 0;
                    d.upload_speed = 0;
                    d.restart_resume = true;
                    Some(d.clone())
                } else {
                    None
                }
            };
            if let Some(d) = snap {
                self.update_download(&d).await;
            }
        }

        // Promote downloads that were moved into top N, limited to available slots
        let active_now = self.active_download_count().await;
        let slots = max_concurrent.saturating_sub(active_now);
        for id in to_start.into_iter().take(slots) {
            // Resume sequentially to avoid race conditions with displacement
            self.resume_download(&id).await;
        }
    }

    pub async fn remove(&self, id: &str) {
        // Cancel any running task first
        self.cancel_download(id).await;

        // Clean up torrent handle and delete from session
        let torrent_id = {
            let mut handles = self.torrent_handles.write().await;
            handles.remove(id)
        };
        if let Some(tid) = torrent_id {
            if let Ok(session) = self.get_torrent_session().await {
                let _ = session.delete(TorrentIdOrHash::Id(tid), false).await;
            }
        }

        // Clean up a persisted .torrent file (qbit-compat adds write it here).
        self.cleanup_persisted_torrent(id).await;

        let id_owned = id.to_string();
        if let Err(e) = self
            .repo_blocking(move |repo| repo.delete_download(&id_owned))
            .await
        {
            tracing::error!("Failed to delete download from DB: {}", e);
        }
        let mut downloads = self.downloads.write().await;
        downloads.remove(id);
    }

    /// Delete the `.torrents/<id>.torrent` sidecar if it exists; best-effort.
    async fn cleanup_persisted_torrent(&self, id: &str) {
        let mut paths = vec![
            persisted_torrent_path(id),
            self.legacy_torrent_path(id).await,
        ];
        if let Some(d) = self
            .get_download(id)
            .await
            .filter(|d| !d.download_folder.is_empty())
        {
            paths.push(
                std::path::Path::new(&d.download_folder)
                    .join(".torrents")
                    .join(format!("{}.torrent", id)),
            );
        }
        for path in paths {
            if path.exists() {
                if let Err(e) = tokio::fs::remove_file(&path).await {
                    tracing::warn!("Failed to delete persisted .torrent {:?}: {}", path, e);
                }
            }
        }
    }

    async fn legacy_torrent_path(&self, id: &str) -> std::path::PathBuf {
        std::path::Path::new(&self.download_dir().await)
            .join(".torrents")
            .join(format!("{}.torrent", id))
    }

    async fn read_persisted_torrent(&self, id: &str) -> std::io::Result<Vec<u8>> {
        match tokio::fs::read(persisted_torrent_path(id)).await {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let folder = self.get_download(id).await.map(|d| d.download_folder);
                if let Some(folder) = folder.filter(|f| !f.is_empty()) {
                    let in_folder = std::path::Path::new(&folder)
                        .join(".torrents")
                        .join(format!("{}.torrent", id));
                    if let Ok(bytes) = tokio::fs::read(in_folder).await {
                        return Ok(bytes);
                    }
                }
                tokio::fs::read(self.legacy_torrent_path(id).await).await
            }
            other => other,
        }
    }

    pub async fn remove_with_files(&self, id: &str) {
        let download = {
            let downloads = self.downloads.read().await;
            downloads.get(id).cloned()
        };

        // Grab the torrent handle BEFORE cancelling — the monitor_torrent cancel handler
        // would otherwise remove it from the map and call session.delete(tid, false) first.
        let torrent_id = {
            let mut handles = self.torrent_handles.write().await;
            handles.remove(id)
        };

        let shared = match &download {
            Some(d) => self.downloads.read().await.values().any(|other| {
                other.id != d.id
                    && (other.save_path == d.save_path
                        || (other.content_path.is_some() && other.content_path == d.content_path))
            }),
            None => false,
        };

        // For torrents: tell librqbit to delete torrent + files. This stops the torrent
        // internally so file handles are released before manual cleanup.
        if let Some(tid) = torrent_id {
            if let Ok(session) = self.get_torrent_session().await {
                if let Err(e) = session.delete(TorrentIdOrHash::Id(tid), !shared).await {
                    tracing::warn!("librqbit delete failed: {}", e);
                }
            }
        }

        // Now cancel the monitor loop (it will see the token and exit;
        // the torrent is already deleted from the session so its delete call is a no-op).
        // Then wait for an HTTP transfer to finish its final writes before deleting files.
        self.cancel_download(id).await;
        let run_lock = self.http_run_lock(id);
        drop(run_lock.lock().await);
        self.http_runs.lock().unwrap().remove(id);

        // Clean up the persisted .torrent sidecar too.
        self.cleanup_persisted_torrent(id).await;

        // Always do manual cleanup: librqbit only deletes individual tracked files,
        // leaving behind the torrent folder and any untracked content (partial files,
        // padding files, etc). remove_dir_all ensures a complete wipe.
        if let Some(ref d) = download {
            let path = std::path::Path::new(&d.save_path);

            let safe = {
                let settings = self.settings.read().await;
                let folders = &settings.download_folders;
                let download_dir = &settings.download_dir;
                match path.canonicalize() {
                    Ok(canonical) => {
                        strictly_inside(&canonical, download_dir)
                            || folders.iter().any(|f| strictly_inside(&canonical, &f.path))
                    }
                    Err(_) => false,
                }
            };

            let owns_files = !shared
                && (d.content_path.is_some()
                    || d.downloaded_size > 0
                    || d.status == DownloadStatus::Completed);
            if safe && owns_files && path.exists() {
                if path.is_dir() {
                    if let Err(e) = tokio::fs::remove_dir_all(path).await {
                        tracing::warn!("Failed to delete directory {}: {}", d.save_path, e);
                    }
                } else if let Err(e) = tokio::fs::remove_file(path).await {
                    tracing::warn!("Failed to delete file {}: {}", d.save_path, e);
                }
                let _ = tokio::fs::remove_file(crate::worker::http::sidecar_path(path)).await;
            } else if !safe {
                tracing::warn!(
                    "Refusing to delete file outside download directory: {}",
                    d.save_path
                );
            }
        }

        // Clean up from DB and in-memory state
        let id_owned = id.to_string();
        if let Err(e) = self
            .repo_blocking(move |repo| repo.delete_download(&id_owned))
            .await
        {
            tracing::error!("Failed to delete download from DB: {}", e);
        }
        let mut downloads = self.downloads.write().await;
        downloads.remove(id);
    }

    async fn register_cancel_token(&self, id: &str) -> CancellationToken {
        let token = CancellationToken::new();
        let mut tokens = self.cancel_tokens.write().await;
        // Cancel any previous token to prevent orphaned tasks
        if let Some(old) = tokens.insert(id.to_string(), token.clone()) {
            old.cancel();
        }
        token
    }

    pub async fn cancel_download(&self, id: &str) {
        let mut tokens = self.cancel_tokens.write().await;
        if let Some(token) = tokens.remove(id) {
            token.cancel();
        }
    }

    pub async fn pause_download(&self, id: &str) {
        // Check if this is a torrent with a live handle — use native pause
        let torrent_id = {
            let handles = self.torrent_handles.read().await;
            handles.get(id).copied()
        };

        if let Some(tid) = torrent_id {
            if let Ok(session) = self.get_torrent_session().await {
                if let Some(handle) = session.get(TorrentIdOrHash::Id(tid)) {
                    if let Err(e) = session.pause(&handle).await {
                        tracing::error!("Failed to pause torrent via librqbit: {}", e);
                    }
                }
            }
        } else {
            // HTTP download — cancel the token (no native pause support)
            self.cancel_download(id).await;
        }

        let download = {
            let mut downloads = self.downloads.write().await;
            if let Some(d) = downloads.get_mut(id) {
                if matches!(
                    d.status,
                    DownloadStatus::Downloading
                        | DownloadStatus::Seeding
                        | DownloadStatus::Fetching
                        | DownloadStatus::Queued
                ) {
                    d.status = DownloadStatus::Paused;
                    d.speed = 0;
                    d.upload_speed = 0;
                    d.restart_resume = false;
                }
                Some(d.clone())
            } else {
                None
            }
        };
        if let Some(d) = download {
            if let Err(e) = self
                .repo_blocking(move |repo| repo.update_download(&d))
                .await
            {
                tracing::error!("Failed to persist pause state: {}", e);
            }
        }
    }

    pub async fn resume_download(self: &Arc<Self>, id: &str) {
        let download = {
            let downloads = self.downloads.read().await;
            downloads.get(id).cloned()
        };
        let Some(d) = download else { return };

        if d.status != DownloadStatus::Paused
            && d.status != DownloadStatus::Failed
            && d.status != DownloadStatus::Stopped
            && d.status != DownloadStatus::Queued
        {
            return;
        }

        // Serialize capacity check + displacement + slot claim with the rest of
        // the queue-management paths so max_concurrent cannot be exceeded.
        let (displaced_id, can_start) = {
            let _slot = self.slot_lock.lock().await;
            let still_resumable = self.downloads.read().await.get(id).is_some_and(|d| {
                matches!(
                    d.status,
                    DownloadStatus::Paused
                        | DownloadStatus::Failed
                        | DownloadStatus::Stopped
                        | DownloadStatus::Queued
                )
            });
            if !still_resumable {
                return;
            }
            let max_concurrent = self.settings.read().await.max_concurrent as usize;
            let active = self.active_download_count().await;
            let displaced = if active >= max_concurrent {
                let lowest_priority_id = {
                    let downloads = self.downloads.read().await;
                    let mut active_list: Vec<_> = downloads
                        .values()
                        .filter(|d| {
                            d.id != id && d.status == DownloadStatus::Downloading
                            // Skip Seeding — arr stack needs pausedUP to import/delete
                        })
                        .collect();
                    active_list.sort_by_key(|d| d.position);
                    active_list.last().map(|d| d.id.clone())
                };
                if let Some(ref oldest_id) = lowest_priority_id {
                    // Flip the victim's status under the slot lock; the slower
                    // librqbit pause + DB persist runs after the lock is released.
                    let mut downloads = self.downloads.write().await;
                    if let Some(d) = downloads.get_mut(oldest_id) {
                        d.status = DownloadStatus::Queued;
                        d.restart_resume = true;
                    }
                }
                lowest_priority_id
            } else {
                None
            };
            let can_start = active < max_concurrent || displaced.is_some();
            if let Some(item) = self.downloads.write().await.get_mut(id) {
                item.status = if can_start {
                    DownloadStatus::Downloading
                } else {
                    DownloadStatus::Queued
                };
            }
            (displaced, can_start)
        };
        if !can_start {
            if let Some(queued) = self.get_download(id).await {
                self.update_download(&queued).await;
            }
            return;
        }
        if let Some(oldest_id) = displaced_id {
            tracing::info!(
                "Queue full: displacing lowest-priority active download {} to make room for {}",
                oldest_id,
                id
            );
            self.pause_download(&oldest_id).await;
            let snap = {
                let downloads = self.downloads.read().await;
                downloads.get(&oldest_id).cloned()
            };
            if let Some(mut d) = snap {
                // pause_download may have flipped the status back to Paused;
                // restore the Queued intent so try_start_queued can promote it.
                d.status = DownloadStatus::Queued;
                d.restart_resume = true;
                self.update_download(&d).await;
            }
        }

        self.launch(id, d).await;
    }

    async fn launch(self: &Arc<Self>, id: &str, d: Download) {
        // Try native librqbit unpause first (torrent was paused, not removed)
        let torrent_id = {
            let handles = self.torrent_handles.read().await;
            handles.get(id).copied()
        };

        if let Some(tid) = torrent_id {
            if let Ok(session) = self.get_torrent_session().await {
                if let Some(handle) = session.get(TorrentIdOrHash::Id(tid)) {
                    if handle.is_paused() {
                        if let Err(e) = session.unpause(&handle).await {
                            tracing::error!("Failed to unpause torrent: {}", e);
                            // Fall through to start_download as fallback
                        } else {
                            // Successfully unpaused — update status and re-monitor
                            {
                                let mut downloads = self.downloads.write().await;
                                if let Some(dl) = downloads.get_mut(id) {
                                    dl.status = DownloadStatus::Downloading;
                                    dl.error_message = None;
                                    dl.restart_resume = true;
                                }
                            }
                            let mut d = d.clone();
                            d.status = DownloadStatus::Downloading;
                            d.error_message = None;
                            d.restart_resume = true;
                            self.update_download(&d).await;

                            // Spawn a monitoring task
                            let state = Arc::clone(self);
                            let cancel_token = state.register_cancel_token(id).await;
                            let id = id.to_string();
                            tokio::spawn(async move {
                                state.monitor_torrent(id, tid, cancel_token).await;
                            });
                            return;
                        }
                    }
                }
            }
        }

        // Fallback: re-start from scratch (for HTTP, failed torrents, etc.)
        let state = Arc::clone(self);
        state.start_download(d).await;
    }

    /// Resume multiple downloads with a staggered delay between each to avoid
    /// overwhelming the system (especially librqbit session init).
    pub async fn resume_all_downloads(self: &Arc<Self>, ids: Vec<String>, delay_secs: u64) {
        let total = ids.len();
        tracing::info!(
            "Resuming {} downloads with {}s delay between each",
            total,
            delay_secs
        );

        for (i, id) in ids.iter().enumerate() {
            let should_resume = {
                let downloads = self.downloads.read().await;
                downloads.get(id).is_some_and(|d| {
                    d.status == DownloadStatus::Paused
                        || d.status == DownloadStatus::Failed
                        || d.status == DownloadStatus::Stopped
                        || d.status == DownloadStatus::Queued
                })
            };

            if should_resume {
                tracing::info!("Resuming download {}/{}: {}", i + 1, total, id);
                self.resume_download(id).await;

                // Stagger: wait between each resume (except the last one)
                if i + 1 < total {
                    tokio::time::sleep(std::time::Duration::from_secs(delay_secs)).await;
                }
            }
        }

        tracing::info!("Resume complete: processed {} downloads", total);
    }

    pub async fn start_download(self: Arc<Self>, download: Download) {
        let state = Arc::clone(&self);
        let cancel_token = state.register_cancel_token(&download.id).await;

        tokio::spawn(async move {
            let protocol = crate::worker::detect_protocol(&download.url);
            let mut download = download;
            download.protocol = protocol.clone();
            download.status = DownloadStatus::Downloading;
            download.restart_resume = true;

            let started = download.clone();
            if let Some(current) = state
                .patch_download(&download.id, |d| {
                    d.protocol = started.protocol.clone();
                    d.status = DownloadStatus::Downloading;
                    d.restart_resume = true;
                })
                .await
            {
                download = current;
            }

            match protocol {
                Protocol::Http => {
                    state
                        .clone()
                        .handle_http_download(download, cancel_token)
                        .await;
                }
                Protocol::Torrent => {
                    state
                        .clone()
                        .handle_torrent_download(download, cancel_token)
                        .await;
                }
                _ => {
                    let mut failed = download;
                    failed.status = DownloadStatus::Failed;
                    failed.error_message = Some("Protocol not yet supported".to_string());
                    state.update_download(&failed).await;
                }
            }
        });
    }

    async fn handle_http_download(
        self: Arc<Self>,
        download: Download,
        cancel_token: CancellationToken,
    ) {
        let (max_conns, min_split_size) = {
            let settings = self.settings.read().await;
            (
                settings.max_connections_per_file as usize,
                settings.min_split_size as u64,
            )
        };

        let run_lock = self.http_run_lock(&download.id);
        let _run = run_lock.lock().await;

        let worker = HttpDownloader::new(
            download.clone(),
            max_conns,
            min_split_size,
            cancel_token.clone(),
        );
        let downloaded_atomic = Arc::clone(&worker.downloaded);
        let active_conns_atomic = Arc::clone(&worker.active_conns);
        let total_size_atomic = Arc::clone(&worker.total_size);
        let save_path_slot = Arc::clone(&worker.save_path);
        let download_id = download.id.clone();

        let mut download_task = tokio::spawn(async move {
            let mut worker = worker;
            worker.run().await
        });

        let apply_save_path = |d: &mut Download| {
            if let Some(path) = save_path_slot.lock().unwrap().clone() {
                d.filename = std::path::Path::new(&path)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                d.save_path = path;
            }
        };

        let mut meter = SpeedMeter::default();
        let mut db_tick: u32 = 0;
        let mut path_saved = false;

        let joined = loop {
            tokio::select! {
                r = &mut download_task => break r,
                _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => {}
            }

            if cancel_token.is_cancelled() {
                match tokio::time::timeout(std::time::Duration::from_secs(10), &mut download_task)
                    .await
                {
                    Ok(r) => break r,
                    Err(_) => {
                        download_task.abort();
                        break Ok(Err(anyhow::anyhow!("Download cancelled")));
                    }
                }
            }

            let current_downloaded = downloaded_atomic.load(std::sync::atomic::Ordering::Relaxed);
            let current_conns = active_conns_atomic.load(std::sync::atomic::Ordering::Relaxed);
            let current_total = total_size_atomic.load(std::sync::atomic::Ordering::Relaxed);
            let speed = if current_conns == 0 {
                meter = SpeedMeter::default();
                0
            } else {
                meter.sample(current_downloaded)
            };

            let snapshot = {
                let mut downloads = self.downloads.write().await;
                if let Some(d) = downloads.get_mut(&download_id) {
                    apply_save_path(d);
                    d.downloaded_size = current_downloaded;
                    d.connections = current_conns;
                    d.speed = speed;
                    if current_total > 0 {
                        d.total_size = current_total;
                    }
                    if d.total_size > 0 {
                        d.progress = (current_downloaded as f64 / d.total_size as f64) * 100.0;
                        d.eta = d
                            .total_size
                            .saturating_sub(current_downloaded)
                            .checked_div(speed)
                            .map(format_eta);
                    }
                    Some(d.clone())
                } else {
                    None
                }
            };

            let persist_path = !path_saved && save_path_slot.lock().unwrap().is_some();
            path_saved |= persist_path;
            db_tick += 1;
            if db_tick >= 5 || persist_path {
                db_tick = 0;
                if let Some(snap) = snapshot {
                    let repo = Arc::clone(&self.repo);
                    tokio::task::spawn_blocking(move || {
                        let _ = repo.update_download(&snap);
                    });
                }
            }
        };

        if cancel_token.is_cancelled() && !matches!(joined, Ok(Ok(_))) {
            let downloaded = downloaded_atomic.load(std::sync::atomic::Ordering::Relaxed);
            self.patch_download(&download_id, |d| {
                apply_save_path(d);
                d.downloaded_size = downloaded;
                if d.total_size > 0 {
                    d.progress = (downloaded as f64 / d.total_size as f64) * 100.0;
                }
                d.speed = 0;
                d.connections = 0;
                d.eta = None;
            })
            .await;
            return;
        }

        match joined {
            Ok(Ok(result)) => {
                let done = result.download;
                let Some(completed) = self
                    .patch_download(&download_id, |d| {
                        d.filename = done.filename.clone();
                        d.save_path = done.save_path.clone();
                        d.status = DownloadStatus::Completed;
                        d.progress = 100.0;
                        d.total_size = done.total_size;
                        d.downloaded_size = done.total_size;
                        d.speed = 0;
                        d.connections = 0;
                        d.eta = None;
                        d.error_message = None;
                        d.completed_at = Some(chrono::Utc::now());
                        d.restart_resume = false;
                    })
                    .await
                else {
                    return;
                };

                // If the downloaded file is a .torrent, auto-start it
                if result.is_torrent_file {
                    tracing::info!(
                        "Detected .torrent file: {}, auto-starting torrent download",
                        completed.save_path
                    );

                    let torrent_path = completed.save_path.clone();
                    let original_id = completed.id.clone();

                    match tokio::fs::read(&torrent_path).await {
                        Ok(torrent_bytes) => {
                            let folder = if completed.download_folder.is_empty() {
                                self.download_dir().await
                            } else {
                                completed.download_folder.clone()
                            };
                            let added = self
                                .add_torrent_from_bytes(
                                    torrent_bytes,
                                    &folder,
                                    completed.category.clone(),
                                )
                                .await;
                            if added.is_some() {
                                if let Err(e) = tokio::fs::remove_file(&torrent_path).await {
                                    tracing::warn!("Failed to delete .torrent file: {}", e);
                                }
                                self.remove(&original_id).await;
                            } else {
                                tracing::warn!(
                                    "Downloaded .torrent {} is invalid or already in downloads",
                                    torrent_path
                                );
                            }
                        }
                        Err(e) => {
                            tracing::error!("Failed to read .torrent file: {}", e);
                        }
                    }
                }
            }
            Ok(Err(e)) => {
                let message = format!("{:#}", e);
                self.mark_http_failed(&download_id, message).await;
            }
            Err(_) => {
                self.mark_http_failed(&download_id, "Download task aborted".to_string())
                    .await;
            }
        }
    }

    pub async fn shutdown(&self) {
        let ids: Vec<String> = self
            .downloads
            .read()
            .await
            .values()
            .filter(|d| d.protocol == Protocol::Http && d.status == DownloadStatus::Downloading)
            .map(|d| d.id.clone())
            .collect();
        for id in &ids {
            self.cancel_download(id).await;
        }
        let drained = async {
            for id in &ids {
                drop(self.http_run_lock(id).lock().await);
            }
        };
        if tokio::time::timeout(std::time::Duration::from_secs(8), drained)
            .await
            .is_err()
        {
            tracing::warn!("Some HTTP downloads did not stop within 8s");
        }
    }

    fn http_run_lock(&self, id: &str) -> Arc<tokio::sync::Mutex<()>> {
        Arc::clone(
            self.http_runs
                .lock()
                .unwrap()
                .entry(id.to_string())
                .or_default(),
        )
    }

    pub async fn set_share_limits(
        &self,
        id: &str,
        ratio_limit: Option<Option<f64>>,
        seeding_time_limit: Option<Option<i64>>,
    ) {
        self.patch_download(id, |d| {
            if let Some(limit) = ratio_limit {
                d.ratio_limit = limit;
            }
            if let Some(limit) = seeding_time_limit {
                d.seeding_time_limit = limit;
            }
        })
        .await;
    }

    async fn mark_http_failed(&self, id: &str, message: String) {
        self.patch_download(id, |d| {
            d.status = DownloadStatus::Failed;
            d.restart_resume = false;
            d.error_message = Some(message);
            d.speed = 0;
            d.connections = 0;
            d.eta = None;
        })
        .await;
    }

    async fn patch_download(&self, id: &str, f: impl FnOnce(&mut Download)) -> Option<Download> {
        let snapshot = {
            let mut downloads = self.downloads.write().await;
            let d = downloads.get_mut(id)?;
            f(d);
            d.clone()
        };
        self.update_download(&snapshot).await;
        Some(snapshot)
    }

    /// Start a torrent download from raw .torrent file bytes.
    /// Used by the qBittorrent compat API for .torrent file uploads.
    pub async fn start_torrent_from_bytes(
        self: Arc<Self>,
        mut download: Download,
        torrent_bytes: Vec<u8>,
    ) {
        let state = Arc::clone(&self);
        let cancel_token = state.register_cancel_token(&download.id).await;

        tokio::spawn(async move {
            download.protocol = Protocol::Torrent;
            download.status = DownloadStatus::Downloading;
            download.restart_resume = true;

            // Persist the torrent file first so it survives restarts
            if let Err(e) = write_persisted_torrent(&download.id, &torrent_bytes).await {
                tracing::warn!("Failed to persist .torrent file: {}", e);
            }

            if let Some(current) = state
                .patch_download(&download.id, |d| {
                    d.protocol = Protocol::Torrent;
                    d.status = DownloadStatus::Downloading;
                    d.restart_resume = true;
                })
                .await
            {
                download = current;
            }

            let add = librqbit::AddTorrent::from_bytes(torrent_bytes);
            if let Err(e) = session_add_and_wait(&state, add, &mut download, &cancel_token).await {
                let current_status = {
                    let downloads = state.downloads.read().await;
                    downloads.get(&download.id).map(|d| d.status.clone())
                };
                match current_status {
                    Some(DownloadStatus::Paused)
                    | Some(DownloadStatus::Stopped)
                    | Some(DownloadStatus::Completed)
                    | Some(DownloadStatus::Seeding) => {}
                    _ => {
                        let message = format!("Torrent failed: {}", e);
                        state
                            .patch_download(&download.id, |d| {
                                d.status = DownloadStatus::Failed;
                                d.restart_resume = false;
                                d.error_message = Some(message);
                            })
                            .await;
                    }
                }
            }
        });
    }

    /// Returns the raw .torrent bytes for a download, if its librqbit session handle is live.
    #[allow(dead_code)]
    pub async fn export_torrent_bytes(&self, id: &str) -> Option<bytes::Bytes> {
        let torrent_id = {
            let handles = self.torrent_handles.read().await;
            handles.get(id).copied()
        };

        // If handle is live, try getting from librqbit session metadata directly
        if let Some(tid) = torrent_id {
            if let Some(session) = {
                let guard = self.torrent_session.read().await;
                guard.as_ref().map(|(_, s)| s.clone())
            } {
                if let Some(handle) = session.get(TorrentIdOrHash::Id(tid)) {
                    if let Ok(bytes) = handle.with_metadata(|m| m.torrent_bytes.clone()) {
                        return Some(bytes);
                    }
                }
            }
        }

        // If not live, check persisted .torrents directory
        self.read_persisted_torrent(id)
            .await
            .ok()
            .map(bytes::Bytes::from)
    }

    pub async fn get_download(&self, id: &str) -> Option<Download> {
        let downloads = self.downloads.read().await;
        downloads.get(id).cloned()
    }

    /// Start an HTTP mirror for a torrent download.
    /// Downloads file(s) via HTTP, then re-adds the torrent for hash verification.
    pub async fn start_http_mirror(
        self: Arc<Self>,
        id: String,
        mirror_url: String,
        keep_seeding: bool,
    ) {
        let state = Arc::clone(&self);

        tokio::spawn(async move {
            if let Err(e) = state
                .run_http_mirror(id.clone(), mirror_url, keep_seeding)
                .await
            {
                tracing::error!("HTTP mirror failed for {}: {}", id, e);

                // Clean up any orphaned torrent handle (if re-add succeeded but later steps failed)
                let orphaned_tid = {
                    let mut handles = state.torrent_handles.write().await;
                    handles.remove(&id)
                };
                if let Some(tid) = orphaned_tid {
                    if let Ok(session) = state.get_torrent_session().await {
                        let _ = session.delete(TorrentIdOrHash::Id(tid), false).await;
                    }
                }

                // Clear mirror status and set error
                let snap = {
                    let mut downloads = state.downloads.write().await;
                    if let Some(d) = downloads.get_mut(&id) {
                        d.http_mirror_status = None;
                        d.http_mirror_url = None;
                        d.error_message = Some(format!("HTTP mirror failed: {}", e));
                        Some(d.clone())
                    } else {
                        None
                    }
                };
                if let Some(snap) = snap {
                    state.update_download(&snap).await;
                }
            }
        });
    }

    async fn run_http_mirror(
        self: &Arc<Self>,
        id: String,
        mirror_url: String,
        keep_seeding: bool,
    ) -> anyhow::Result<()> {
        use crate::worker::mirror::{extract_zip_safe, MirrorDownloader};

        // 1. Atomically validate download and set mirror status (prevents race condition)
        let (snapshot_save_path, snapshot_content_path, snapshot_filename) = {
            let mut downloads = self.downloads.write().await;
            let download = downloads
                .get_mut(&id)
                .ok_or_else(|| anyhow::anyhow!("Download not found"))?;

            if download.protocol != Protocol::Torrent {
                anyhow::bail!("Not a torrent download");
            }
            if download.http_mirror_status.is_some() {
                anyhow::bail!("Mirror already in progress");
            }

            // Snapshot arr-safe fields
            let snap = (
                download.save_path.clone(),
                download.content_path.clone(),
                download.filename.clone(),
            );

            // Set mirror status atomically with the check
            download.http_mirror_status = Some("downloading".to_string());
            download.http_mirror_url = Some(mirror_url.clone());

            snap
        };

        // 2. Determine output_dir for re-add (parent of save_path, not global download_dir)
        let output_dir = match std::path::Path::new(&snapshot_save_path).parent() {
            Some(p) => p.to_string_lossy().to_string(),
            None => self.download_dir().await,
        };

        // 3. Persist mirror status to DB immediately (crash safety: if we crash after
        // removing the torrent from the session, startup recovery will see mirror status
        // and know to clean up rather than leaving an orphaned download)
        if let Some(snap) = self.get_download(&id).await {
            self.update_download(&snap).await;
        }

        // 4. Cancel any existing monitoring token FIRST (so monitor_torrent exits cleanly)
        self.cancel_download(&id).await;
        tokio::task::yield_now().await;

        // 5. Pause torrent in librqbit and delete from session (keep files)
        let torrent_id = {
            let mut handles = self.torrent_handles.write().await;
            handles.remove(&id)
        };
        let mut readd_dir = output_dir.clone();
        if let Some(tid) = torrent_id {
            if let Ok(session) = self.get_torrent_session().await {
                if let Some(handle) = session.get(TorrentIdOrHash::Id(tid)) {
                    let layout = handle.with_metadata(|m| {
                        crate::domain::classify_layout(
                            m.file_infos
                                .iter()
                                .filter(|f| !f.attrs.padding)
                                .map(|f| f.relative_filename.clone()),
                        )
                    });
                    let folder = self
                        .get_download(&id)
                        .await
                        .map(|d| d.download_folder)
                        .filter(|f| !f.is_empty())
                        .unwrap_or_else(|| output_dir.clone());
                    if let Ok(layout) = layout {
                        readd_dir = crate::domain::compute_torrent_paths(
                            &folder,
                            &snapshot_filename,
                            &layout,
                        )
                        .output_folder;
                    }
                    let _ = session.pause(&handle).await;
                }
                let _ = session.delete(TorrentIdOrHash::Id(tid), false).await;
            }
        }
        if readd_dir == output_dir {
            if let Some(d) = self
                .get_download(&id)
                .await
                .filter(|d| !d.download_folder.is_empty())
            {
                match self.listed_output_folder(&d).await {
                    Ok(folder) => readd_dir = folder,
                    Err(e) => tracing::warn!("Could not read torrent layout for mirror: {}", e),
                }
            }
        }

        // 6. Register cancel token for HTTP download phase
        let http_cancel = self.register_cancel_token(&id).await;
        let was_cancelled = http_cancel.clone();

        // 7. Always download to a temp file first.
        let tmp_dir = format!("{}/.tmp", output_dir);
        let tmp_download_path = format!("{}/{}.mirror", tmp_dir, id);
        tokio::fs::create_dir_all(&tmp_dir).await?;

        let target_path = snapshot_content_path
            .as_deref()
            .unwrap_or(&snapshot_save_path);

        let downloader = MirrorDownloader::new(
            mirror_url.clone(),
            tmp_download_path.clone(),
            http_cancel.clone(),
        );
        let downloaded_atomic = Arc::clone(&downloader.downloaded);
        let total_size_atomic = Arc::clone(&downloader.total_size);

        let download_task = tokio::spawn(async move { downloader.run().await });

        // 8. Monitor loop
        let mut meter = SpeedMeter::default();
        let mut db_tick: u32 = 0;

        loop {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;

            if download_task.is_finished() {
                break;
            }

            if http_cancel.is_cancelled() {
                download_task.abort();
                break;
            }

            let current_downloaded = downloaded_atomic.load(std::sync::atomic::Ordering::Relaxed);
            let current_total = total_size_atomic.load(std::sync::atomic::Ordering::Relaxed);
            let speed = meter.sample(current_downloaded);

            {
                let mut downloads = self.downloads.write().await;
                if let Some(d) = downloads.get_mut(&id) {
                    d.downloaded_size = current_downloaded;
                    d.speed = speed;
                    d.upload_speed = 0;
                    if current_total > 0 {
                        d.total_size = current_total;
                        d.progress = (current_downloaded as f64 / current_total as f64) * 100.0;
                        d.eta = current_total
                            .saturating_sub(current_downloaded)
                            .checked_div(speed)
                            .map(format_eta);
                    }
                }
            }

            db_tick += 1;
            if db_tick >= 5 {
                db_tick = 0;
                if let Some(snap) = self.get_download(&id).await {
                    let repo = Arc::clone(&self.repo);
                    tokio::task::spawn_blocking(move || {
                        let _ = repo.update_download(&snap);
                    });
                }
            }
        }

        // 9. Handle download result
        let (http_succeeded, is_zip) = match download_task.await {
            Ok(Ok(result)) => (true, result.is_zip),
            Ok(Err(e)) => {
                tracing::warn!(
                    "HTTP mirror download error (will still re-add torrent): {}",
                    e
                );
                (false, false)
            }
            Err(_) => {
                tracing::warn!("HTTP mirror task aborted (will still re-add torrent)");
                (false, false)
            }
        };

        // 10. Move or extract the downloaded file
        if http_succeeded {
            if is_zip {
                {
                    let mut downloads = self.downloads.write().await;
                    if let Some(d) = downloads.get_mut(&id) {
                        d.http_mirror_status = Some("extracting".to_string());
                    }
                }

                let tmp_zip_path = format!("{}/{}.zip", tmp_dir, id);
                let _ = tokio::fs::rename(&tmp_download_path, &tmp_zip_path).await;

                let extract_target = readd_dir.clone();
                let zip_path = tmp_zip_path.clone();
                match tokio::task::spawn_blocking(move || {
                    extract_zip_safe(&zip_path, &extract_target)
                })
                .await
                {
                    Ok(Ok(files)) => {
                        tracing::info!("Extracted {} files from mirror zip", files.len());
                    }
                    Ok(Err(e)) => {
                        tracing::warn!("Zip extraction failed: {}", e);
                    }
                    Err(e) => {
                        tracing::warn!("Zip extraction task panicked: {}", e);
                    }
                }

                let _ = tokio::fs::remove_file(&tmp_zip_path).await;
            } else {
                // Single-file: move temp file to the torrent's target path
                if let Err(e) = tokio::fs::rename(&tmp_download_path, target_path).await {
                    if let Err(e2) = tokio::fs::copy(&tmp_download_path, target_path).await {
                        tracing::warn!("Failed to move mirror file: rename={}, copy={}", e, e2);
                    }
                    let _ = tokio::fs::remove_file(&tmp_download_path).await;
                }
            }
        }
        // Clean up temp file if it still exists
        let _ = tokio::fs::remove_file(&tmp_download_path).await;
        let _ = tokio::fs::remove_dir(&tmp_dir).await;

        // 11. Set rechecking status
        {
            let mut downloads = self.downloads.write().await;
            if let Some(d) = downloads.get_mut(&id) {
                d.http_mirror_status = Some("rechecking".to_string());
                d.speed = 0;
            }
        }

        // 12. Re-add torrent
        let mut download = self
            .get_download(&id)
            .await
            .ok_or_else(|| anyhow::anyhow!("Download disappeared during mirror"))?;

        let torrent_id = self.mirror_readd_torrent(&mut download, &readd_dir).await?;

        // 13. Restore snapshotted fields
        download.save_path = snapshot_save_path;
        download.content_path = snapshot_content_path;
        download.filename = snapshot_filename;
        download.status = DownloadStatus::Downloading;
        download.http_mirror_status = None;
        download.http_mirror_url = None;
        self.update_download(&download).await;

        // 14. If HTTP phase was cancelled, pause the re-added torrent immediately
        if was_cancelled.is_cancelled() {
            if let Ok(session) = self.get_torrent_session().await {
                if let Some(handle) = session.get(TorrentIdOrHash::Id(torrent_id)) {
                    let _ = session.pause(&handle).await;
                }
            }
            let mut d = download;
            d.status = DownloadStatus::Paused;
            d.speed = 0;
            self.update_download(&d).await;
            return Ok(());
        }

        // 15. Register new cancel token and start monitoring
        let monitor_cancel = self.register_cancel_token(&id).await;
        let mut monitor =
            tokio::spawn(Arc::clone(self).monitor_torrent(id.clone(), torrent_id, monitor_cancel));

        // 16. Stop seeding once the recheck completes when keep_seeding is off
        if !keep_seeding {
            loop {
                tokio::select! {
                    _ = &mut monitor => break,
                    _ = tokio::time::sleep(std::time::Duration::from_secs(2)) => {}
                }
                let Some(d) = self.get_download(&id).await else {
                    break;
                };
                if d.status == DownloadStatus::Seeding {
                    self.cancel_download(&id).await;
                    let _ = (&mut monitor).await;
                    self.patch_download(&id, |d| {
                        d.status = DownloadStatus::Completed;
                        d.upload_speed = 0;
                        d.speed = 0;
                    })
                    .await;
                    break;
                }
            }
        }

        Ok(())
    }

    async fn handle_torrent_download(
        self: Arc<Self>,
        mut download: Download,
        cancel_token: CancellationToken,
    ) {
        let url = download.url.clone();
        let is_magnet = url.starts_with("magnet:");
        if is_magnet {
            download.status = DownloadStatus::Fetching;
            self.update_download(&download).await;
        }
        let add = if is_magnet {
            librqbit::AddTorrent::from_url(&url)
        } else if url.starts_with("torrent://") {
            match self.read_persisted_torrent(&download.id).await {
                Ok(bytes) => librqbit::AddTorrent::from_bytes(bytes),
                Err(e) => {
                    download.status = DownloadStatus::Failed;
                    download.restart_resume = false;
                    download.error_message =
                        Some(format!("Failed to read persisted torrent file: {}", e));
                    self.update_download(&download).await;
                    return;
                }
            }
        } else {
            let fetched = tokio::select! {
                r = crate::worker::http::fetch_small(&url, crate::worker::http::MAX_TORRENT_FILE_BYTES) => r,
                _ = cancel_token.cancelled() => return,
            };
            match fetched {
                Ok(bytes) => librqbit::AddTorrent::from_bytes(bytes),
                Err(e) => {
                    download.status = DownloadStatus::Failed;
                    download.restart_resume = false;
                    download.error_message =
                        Some(format!("Failed to fetch .torrent file: {:#}", e));
                    self.update_download(&download).await;
                    return;
                }
            }
        };

        if let Err(e) = session_add_and_wait(&self, add, &mut download, &cancel_token).await {
            // Only mark as failed if not already handled (paused/stopped/completed)
            let current_status = {
                let downloads = self.downloads.read().await;
                downloads.get(&download.id).map(|d| d.status.clone())
            };
            match current_status {
                Some(DownloadStatus::Paused)
                | Some(DownloadStatus::Stopped)
                | Some(DownloadStatus::Completed)
                | Some(DownloadStatus::Seeding) => {
                    // Already handled by monitor_torrent
                }
                _ => {
                    let message = format!("Torrent failed: {}", e);
                    self.patch_download(&download.id, |d| {
                        d.status = DownloadStatus::Failed;
                        d.restart_resume = false;
                        d.error_message = Some(message);
                    })
                    .await;
                }
            }
        }
    }

    // ─── History ────────────────────────────────────────

    pub async fn get_history_page(&self, limit: i64, offset: i64) -> Vec<serde_json::Value> {
        self.repo_blocking(move |repo| repo.get_history_page(limit, offset).unwrap_or_default())
            .await
    }

    pub async fn delete_history(&self, id: &str) {
        let id_owned = id.to_string();
        if let Err(e) = self
            .repo_blocking(move |repo| repo.delete_history(&id_owned))
            .await
        {
            tracing::error!("Failed to delete history entry: {}", e);
        }
    }

    pub async fn delete_all_history(&self) {
        if let Err(e) = self.repo_blocking(|repo| repo.delete_all_history()).await {
            tracing::error!("Failed to clear history: {}", e);
        }
    }
}

impl ManagerState {
    /// Monitor an existing torrent handle (used after adding or unpausing).
    /// Returns when torrent is paused, cancelled, or errors out.
    async fn monitor_torrent(
        self: Arc<Self>,
        download_id: String,
        torrent_id: usize,
        cancel_token: CancellationToken,
    ) {
        let session = match self.get_torrent_session().await {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("Failed to get torrent session for monitoring: {}", e);
                return;
            }
        };

        let handle = match session.get(TorrentIdOrHash::Id(torrent_id)) {
            Some(h) => h,
            None => {
                tracing::error!("Torrent handle {} not found in session", torrent_id);
                let snap = {
                    let mut downloads = self.downloads.write().await;
                    if let Some(d) = downloads.get_mut(&download_id) {
                        d.status = DownloadStatus::Failed;
                        d.restart_resume = false;
                        d.error_message = Some("Torrent handle lost".to_string());
                        Some(d.clone())
                    } else {
                        None
                    }
                };
                if let Some(snap) = snap {
                    let repo = Arc::clone(&self.repo);
                    tokio::task::spawn_blocking(move || {
                        let _ = repo.update_download(&snap);
                    });
                }
                return;
            }
        };

        let mut down_meter = SpeedMeter::default();
        let mut up_meter = SpeedMeter::default();
        let mut last_uploaded: Option<u64> = None;
        let mut db_tick: u32 = 0;

        loop {
            // Check if cancelled (stop/remove)
            if cancel_token.is_cancelled() {
                let superseded = self
                    .cancel_tokens
                    .read()
                    .await
                    .get(&download_id)
                    .is_some_and(|t| !t.is_cancelled());
                if superseded {
                    return;
                }
                let current_status = {
                    let downloads = self.downloads.read().await;
                    downloads.get(&download_id).map(|d| d.status.clone())
                };
                match current_status {
                    Some(DownloadStatus::Paused) => {
                        return;
                    }
                    _ => {
                        let _ = session.delete(handle.id().into(), false).await;
                        {
                            let mut handles = self.torrent_handles.write().await;
                            handles.remove(&download_id);
                        }
                        // Don't set status here — let the caller (remove/cancel) handle final state.
                        // Just clean up speed so the UI doesn't show stale values.
                        {
                            let mut downloads = self.downloads.write().await;
                            if let Some(d) = downloads.get_mut(&download_id) {
                                d.speed = 0;
                                d.upload_speed = 0;
                            }
                        }
                        return;
                    }
                }
            }

            // Check if paused via librqbit
            if handle.is_paused() {
                let snap = {
                    let mut downloads = self.downloads.write().await;
                    if let Some(d) = downloads.get_mut(&download_id) {
                        // Don't overwrite Queued status — reorder_downloads sets Queued
                        // when demoting a download, and the torrent is paused as part of
                        // that transition. We should not revert it back to Paused.
                        if d.status == DownloadStatus::Queued {
                            return;
                        }
                        d.status = DownloadStatus::Paused;
                        d.speed = 0;
                        d.upload_speed = 0;
                        Some(d.clone())
                    } else {
                        None
                    }
                };
                if let Some(snap) = snap {
                    let repo = Arc::clone(&self.repo);
                    tokio::task::spawn_blocking(move || {
                        let _ = repo.update_download(&snap);
                    });
                }
                return;
            }

            let stats = handle.stats();
            let uploaded_delta = match last_uploaded {
                Some(prev) if stats.uploaded_bytes >= prev => stats.uploaded_bytes - prev,
                Some(_) => stats.uploaded_bytes,
                None => 0,
            };
            last_uploaded = Some(stats.uploaded_bytes);
            let (global_ratio_limit, global_time_limit) = {
                let settings = self.settings.read().await;
                (settings.seed_ratio_limit, settings.seed_time_limit)
            };
            let mut stop_seeding = false;

            if matches!(stats.state, librqbit::TorrentStatsState::Paused) && !handle.is_paused() {
                let wants_run = {
                    let downloads = self.downloads.read().await;
                    downloads.get(&download_id).is_some_and(|d| {
                        matches!(
                            d.status,
                            DownloadStatus::Downloading
                                | DownloadStatus::Fetching
                                | DownloadStatus::Seeding
                        )
                    })
                };
                if wants_run {
                    if let Err(e) = session.unpause(&handle).await {
                        tracing::warn!("Failed to restart stalled torrent {}: {}", download_id, e);
                    }
                }
            }

            if let Some(error) = stats.error.clone() {
                let _ = session.delete(handle.id().into(), false).await;
                {
                    let mut handles = self.torrent_handles.write().await;
                    if handles.get(&download_id) == Some(&torrent_id) {
                        handles.remove(&download_id);
                    }
                }
                let snap = {
                    let mut downloads = self.downloads.write().await;
                    downloads.get_mut(&download_id).map(|d| {
                        d.status = DownloadStatus::Failed;
                        d.restart_resume = false;
                        d.error_message = Some(format!("Torrent error: {}", error));
                        d.speed = 0;
                        d.upload_speed = 0;
                        d.eta = None;
                        d.clone()
                    })
                };
                if let Some(snap) = snap {
                    self.update_download(&snap).await;
                }
                return;
            }

            let snapshot = {
                let mut downloads = self.downloads.write().await;
                if let Some(download) = downloads.get_mut(&download_id) {
                    download.total_size = stats.total_bytes;
                    download.downloaded_size = stats.progress_bytes;
                    if stats.total_bytes > 0 {
                        download.progress =
                            (stats.progress_bytes as f64 / stats.total_bytes as f64) * 100.0;
                    }

                    if let Some(live) = &stats.live {
                        download.speed = down_meter.sample(live.snapshot.fetched_bytes);
                        download.upload_speed = up_meter.sample(stats.uploaded_bytes);
                        let ps = &live.snapshot.peer_stats;
                        download.peers = ps.live;
                        download.seeds = ps.seen;
                        download.eta = stats
                            .total_bytes
                            .saturating_sub(stats.progress_bytes)
                            .checked_div(download.speed)
                            .filter(|_| !stats.finished)
                            .map(format_eta);
                    }

                    if download.status == DownloadStatus::Fetching && stats.total_bytes > 0 {
                        download.status = DownloadStatus::Downloading;
                    }

                    download.uploaded_size += uploaded_delta;

                    if stats.finished && download.status != DownloadStatus::Seeding {
                        download.status = DownloadStatus::Seeding;
                        download.speed = 0;
                        download.progress = 100.0;
                        download.completed_at = Some(chrono::Utc::now());
                        download.eta = None;
                    }

                    if download.status == DownloadStatus::Seeding
                        && seed_limit_reached(download, global_ratio_limit, global_time_limit)
                    {
                        stop_seeding = true;
                        download.status = DownloadStatus::Completed;
                        download.speed = 0;
                        download.upload_speed = 0;
                        download.restart_resume = false;
                    }

                    Some(download.clone())
                } else {
                    None
                }
            }; // write lock released

            if stop_seeding {
                let _ = session.pause(&handle).await;
                if let Some(snap) = snapshot {
                    self.update_download(&snap).await;
                }
                return;
            }

            // DB write every 5 ticks (seeding: every 5*2=10s, downloading: every 5s)
            db_tick += 1;
            if db_tick >= 5 {
                db_tick = 0;
                if let Some(snap) = snapshot {
                    let repo = Arc::clone(&self.repo);
                    tokio::task::spawn_blocking(move || {
                        let _ = repo.update_download(&snap);
                    });
                }
            }

            let sleep_dur = if stats.finished { 2 } else { 1 };
            tokio::time::sleep(std::time::Duration::from_secs(sleep_dur)).await;
        }
    }

    /// Re-add a torrent to librqbit after HTTP mirror download.
    /// Unlike `session_add_and_wait`, this does NOT overwrite filename/save_path/content_path
    /// to preserve arr stack data.
    async fn torrent_add_source(
        &self,
        download: &Download,
    ) -> anyhow::Result<librqbit::AddTorrent<'static>> {
        let url = download.url.clone();
        if url.starts_with("magnet:") {
            Ok(librqbit::AddTorrent::from_url(url))
        } else if url.starts_with("torrent://") {
            let bytes = self
                .read_persisted_torrent(&download.id)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to read persisted torrent file: {}", e))?;
            Ok(librqbit::AddTorrent::from_bytes(bytes))
        } else {
            let bytes =
                crate::worker::http::fetch_small(&url, crate::worker::http::MAX_TORRENT_FILE_BYTES)
                    .await?;
            Ok(librqbit::AddTorrent::from_bytes(bytes))
        }
    }

    async fn listed_output_folder(&self, download: &Download) -> anyhow::Result<String> {
        let add = self.torrent_add_source(download).await?;
        let session = self.get_torrent_session().await?;
        let opts = librqbit::AddTorrentOptions {
            list_only: true,
            ..Default::default()
        };
        let librqbit::AddTorrentResponse::ListOnly(listed) = session
            .add_torrent(add, Some(opts))
            .await
            .map_err(|e| anyhow::anyhow!("Failed to read torrent metadata: {}", e))?
        else {
            anyhow::bail!("Failed to read torrent metadata");
        };
        let layout = crate::domain::classify_layout(
            listed
                .info
                .iter_file_details()
                .filter(|f| !f.attrs().padding)
                .map(|f| f.filename.to_pathbuf()),
        );
        Ok(crate::domain::compute_torrent_paths(
            &download.download_folder,
            &download.filename,
            &layout,
        )
        .output_folder)
    }

    async fn mirror_readd_torrent(
        self: &Arc<Self>,
        download: &mut Download,
        output_dir: &str,
    ) -> anyhow::Result<usize> {
        let add = self.torrent_add_source(download).await?;

        let session = self.get_torrent_session().await?;
        let opts = librqbit::AddTorrentOptions {
            overwrite: true,
            output_folder: Some(output_dir.to_string()),
            ..Default::default()
        };

        let handle = session
            .add_torrent(add, Some(opts))
            .await
            .map_err(|e| anyhow::anyhow!("Failed to re-add torrent: {}", e))?
            .into_handle()
            .ok_or_else(|| anyhow::anyhow!("Torrent was a duplicate or couldn't get handle"))?;

        let torrent_id = handle.id();

        // Store handle mapping — but do NOT overwrite filename/save_path/content_path
        {
            let mut handles = self.torrent_handles.write().await;
            handles.insert(download.id.clone(), torrent_id);
        }

        // Only update info_hash if not already set
        let hash_str = handle.info_hash().as_string();
        if download.info_hash.is_none() {
            download.info_hash = Some(hash_str);
        }

        Ok(torrent_id)
    }
}

fn seed_limit_reached(d: &Download, ratio_limit: Option<f64>, time_limit: Option<u64>) -> bool {
    let ratio_limit = match d.ratio_limit {
        Some(limit) if limit >= 0.0 => Some(limit),
        Some(_) => None,
        None => ratio_limit,
    };
    let time_limit = match d.seeding_time_limit {
        Some(limit) if limit >= 0 => Some(limit as u64),
        Some(_) => None,
        None => time_limit,
    };
    let ratio = d.uploaded_size as f64 / d.total_size.max(1) as f64;
    let seeded_minutes = d
        .completed_at
        .map(|c| (chrono::Utc::now() - c).num_minutes().max(0) as u64)
        .unwrap_or(0);
    ratio_limit.is_some_and(|limit| ratio + 0.001 >= limit)
        || time_limit.is_some_and(|limit| seeded_minutes >= limit)
}

fn strictly_inside(path: &std::path::Path, folder: &str) -> bool {
    let folder = std::path::Path::new(folder);
    let folder = folder
        .canonicalize()
        .unwrap_or_else(|_| folder.to_path_buf());
    path != folder && path.starts_with(&folder)
}

fn persisted_torrent_path(id: &str) -> std::path::PathBuf {
    let data_dir = std::env::var("DLOAD_DATA_DIR").unwrap_or_else(|_| "/data".to_string());
    std::path::Path::new(&data_dir)
        .join("torrents")
        .join(format!("{}.torrent", id))
}

async fn write_persisted_torrent(id: &str, bytes: &[u8]) -> std::io::Result<()> {
    let path = persisted_torrent_path(id);
    if let Some(dir) = path.parent() {
        tokio::fs::create_dir_all(dir).await?;
    }
    tokio::fs::write(path, bytes).await
}

async fn session_add_and_wait(
    state: &Arc<ManagerState>,
    add: librqbit::AddTorrent<'_>,
    download: &mut Download,
    cancel_token: &CancellationToken,
) -> anyhow::Result<()> {
    use crate::domain::{classify_layout, compute_torrent_paths};

    let session = state.get_torrent_session().await?;

    // Ensure download_folder is set. Migrations backfill it for existing rows; this branch
    // covers any row that slipped through (empty string) so we never accidentally write to
    // the process default.
    if download.download_folder.is_empty() {
        let fallback = state.download_dir().await;
        let derived = std::path::Path::new(&download.save_path)
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(|p| p.to_string_lossy().to_string());
        download.download_folder = derived.unwrap_or(fallback);
    }
    let download_folder = download.download_folder.clone();

    let magnet_trackers = download
        .url
        .starts_with("magnet:")
        .then(|| librqbit::Magnet::parse(&download.url).ok())
        .flatten()
        .map(|m| m.trackers)
        .filter(|t| !t.is_empty());

    let list_opts = librqbit::AddTorrentOptions {
        list_only: true,
        output_folder: Some(download_folder.clone()),
        ..Default::default()
    };
    let listed = tokio::select! {
        r = session.add_torrent(add, Some(list_opts)) => r
            .map_err(|e| anyhow::anyhow!("Failed to add torrent: {}", e))?,
        _ = cancel_token.cancelled() => return Ok(()),
    };
    let librqbit::AddTorrentResponse::ListOnly(listed) = listed else {
        anyhow::bail!("Failed to read torrent metadata");
    };

    let torrent_name = listed
        .info
        .name()
        .map(|n| crate::domain::sanitize_filename(&n))
        .unwrap_or_else(|| download.filename.clone());
    download.filename = torrent_name.clone();

    let layout = classify_layout(
        listed
            .info
            .iter_file_details()
            .filter(|f| !f.attrs().padding)
            .map(|f| f.filename.to_pathbuf()),
    );
    let paths = compute_torrent_paths(&download_folder, &torrent_name, &layout);

    let add_opts = librqbit::AddTorrentOptions {
        paused: true,
        overwrite: true,
        output_folder: Some(paths.output_folder.clone()),
        initial_peers: Some(listed.seen_peers),
        trackers: magnet_trackers,
        ..Default::default()
    };
    let added = tokio::select! {
        r = session.add_torrent(
            librqbit::AddTorrent::TorrentFileBytes(listed.torrent_bytes),
            Some(add_opts),
        ) => r.map_err(|e| anyhow::anyhow!("Failed to add torrent: {}", e))?,
        _ = cancel_token.cancelled() => return Ok(()),
    };
    let handle = added
        .into_handle()
        .ok_or_else(|| anyhow::anyhow!("Torrent was a duplicate or couldn't get handle"))?;

    let owned_elsewhere = state
        .torrent_handles
        .read()
        .await
        .iter()
        .any(|(id, tid)| *tid == handle.id() && *id != download.id);
    if owned_elsewhere {
        anyhow::bail!("This torrent is already in your downloads");
    }

    let torrent_id = handle.id();

    {
        let mut handles = state.torrent_handles.write().await;
        handles.insert(download.id.clone(), torrent_id);
    }

    // Lock in the invariant: content_path is a strict child of download_folder. save_path
    // mirrors content_path for internal callers that still read it expecting the on-disk
    // location; the qbit-compat API layer derives its own save_path from download_folder.
    download.save_path = paths.content_path.clone();
    download.content_path = Some(paths.content_path.clone());

    let hash_str = handle.info_hash().as_string();
    if download.info_hash.is_none() {
        download.info_hash = Some(hash_str);
    }
    let resolved = download.clone();
    state
        .patch_download(&download.id, |d| {
            d.filename = resolved.filename.clone();
            d.save_path = resolved.save_path.clone();
            d.content_path = resolved.content_path.clone();
            d.info_hash = resolved.info_hash.clone();
            d.download_folder = resolved.download_folder.clone();
        })
        .await;

    // CRITICAL: wait for librqbit to finish initializing (checksum) before unpausing.
    // Calling unpause() while the torrent is still in the Initializing state silently
    // drops the start request (librqbit logs "no need to start torrent anymore, as it
    // switched state from initializing"), leaving the torrent paused forever with zero
    // peers. Re-read the status afterwards: the user may have paused, stopped, demoted
    // or removed the download while metadata resolved or the checksum ran.
    let wants_to_run = |status: Option<&DownloadStatus>| {
        matches!(
            status,
            Some(DownloadStatus::Downloading | DownloadStatus::Fetching)
        )
    };
    loop {
        tokio::select! {
            _ = handle.wait_until_initialized() => break,
            _ = cancel_token.cancelled() => break,
            _ = tokio::time::sleep(std::time::Duration::from_secs(2)) => {
                let downloads = state.downloads.read().await;
                if !wants_to_run(downloads.get(&download.id).map(|d| &d.status)) {
                    break;
                }
            }
        }
    }

    let current_status = {
        let downloads = state.downloads.read().await;
        downloads.get(&download.id).map(|d| d.status.clone())
    };
    if cancel_token.is_cancelled() || current_status.is_none() {
        let superseded = state
            .cancel_tokens
            .read()
            .await
            .get(&download.id)
            .is_some_and(|t| !t.is_cancelled());
        if !superseded {
            let _ = session.delete(handle.id().into(), false).await;
            let mut handles = state.torrent_handles.write().await;
            if handles.get(&download.id) == Some(&torrent_id) {
                handles.remove(&download.id);
            }
        }
        return Ok(());
    }
    let run = wants_to_run(current_status.as_ref());
    if run {
        if let Err(e) = session.unpause(&handle).await {
            tracing::error!("Failed to unpause torrent after metadata resolve: {}", e);
        }
    }

    if !run {
        return Ok(());
    }
    state
        .patch_download(&download.id, |d| {
            d.status = DownloadStatus::Downloading;
            d.error_message = None;
        })
        .await;

    let state_clone = Arc::clone(state);
    let dl_id = download.id.clone();
    let ct = cancel_token.clone();
    state_clone.monitor_torrent(dl_id, torrent_id, ct).await;

    let latest = {
        let downloads = state.downloads.read().await;
        downloads.get(&download.id).cloned()
    };
    if let Some(latest) = latest {
        *download = latest;
    }

    Ok(())
}

pub type SharedState = Arc<ManagerState>;

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderMap;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::time::Duration;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(
                axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                v.parse().unwrap(),
            );
        }
        h
    }

    #[test]
    fn seed_limits_prefer_torrent_values_over_global() {
        let mut d = Download::new(
            "magnet:?xt=urn:btih:0000000000000000000000000000000000000000".into(),
            "/tmp",
        );
        d.total_size = 100;
        d.uploaded_size = 50;
        d.completed_at = Some(chrono::Utc::now());
        assert!(!seed_limit_reached(&d, None, None));
        assert!(seed_limit_reached(&d, Some(0.5), None));
        assert!(!seed_limit_reached(&d, Some(1.0), None));
        d.ratio_limit = Some(-1.0);
        assert!(!seed_limit_reached(&d, Some(0.0), None));
        d.ratio_limit = Some(0.25);
        assert!(seed_limit_reached(&d, None, None));
        d.ratio_limit = None;
        d.completed_at = Some(chrono::Utc::now() - chrono::Duration::minutes(30));
        assert!(seed_limit_reached(&d, None, Some(30)));
        assert!(!seed_limit_reached(&d, None, Some(31)));
    }

    #[tokio::test(start_paused = true)]
    async fn speed_meter_averages_over_window_and_resets_on_counter_drop() {
        let mut meter = SpeedMeter::default();
        assert_eq!(meter.sample(0), 0);
        for (i, bytes) in [10, 40, 50, 80].iter().enumerate() {
            tokio::time::advance(Duration::from_secs(1)).await;
            let speed = meter.sample(bytes * 1_000_000);
            if i == 3 {
                assert_eq!(speed, 20_000_000);
            }
        }
        tokio::time::advance(Duration::from_secs(1)).await;
        assert_eq!(meter.sample(5), 0);
    }

    // ─── extract_client_ip ────────────────────────────────────────────────

    #[test]
    fn uses_x_forwarded_for_hop_added_by_proxy() {
        let h = headers(&[("x-forwarded-for", "1.2.3.4, 203.0.113.5")]);
        assert_eq!(
            extract_client_ip_inner(&h, None, true).unwrap(),
            "203.0.113.5".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn falls_through_to_x_real_ip_when_xff_missing() {
        let h = headers(&[("x-real-ip", "198.51.100.42")]);
        assert_eq!(
            extract_client_ip_inner(&h, None, true).unwrap(),
            "198.51.100.42".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn falls_back_to_socket_peer_when_no_proxy_headers() {
        let peer = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4)), 12345);
        let h = HeaderMap::new();
        assert_eq!(
            extract_client_ip_inner(&h, Some(peer), false).unwrap(),
            "1.2.3.4".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn returns_none_when_nothing_available() {
        assert_eq!(
            extract_client_ip_inner(&HeaderMap::new(), None, false),
            None
        );
    }

    #[test]
    fn ignores_malformed_xff() {
        let h = headers(&[("x-forwarded-for", "10.0.0.1, not-an-ip")]);
        // Last token is malformed → extractor returns None from the XFF branch;
        // with no X-Real-IP and no peer, we get None overall
        assert_eq!(extract_client_ip_inner(&h, None, true), None);
    }

    #[test]
    fn ignores_proxy_headers_when_trust_proxy_disabled() {
        let h = headers(&[("x-forwarded-for", "203.0.113.5")]);
        let peer = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), 12345);
        assert_eq!(
            extract_client_ip_inner(&h, Some(peer), false).unwrap(),
            "10.0.0.1".parse::<IpAddr>().unwrap()
        );
    }

    // ─── LoginRateLimiter ─────────────────────────────────────────────────

    #[tokio::test(start_paused = true)]
    async fn rate_limiter_allows_up_to_max_attempts_per_ip() {
        let limiter = LoginRateLimiter::new(3, Duration::from_secs(60));
        let ip: IpAddr = "1.2.3.4".parse().unwrap();
        assert!(limiter.try_consume(ip).await);
        assert!(limiter.try_consume(ip).await);
        assert!(limiter.try_consume(ip).await);
        assert!(
            !limiter.try_consume(ip).await,
            "4th attempt should be blocked"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn rate_limiter_isolates_per_ip() {
        let limiter = LoginRateLimiter::new(2, Duration::from_secs(60));
        let a: IpAddr = "1.1.1.1".parse().unwrap();
        let b: IpAddr = "2.2.2.2".parse().unwrap();
        assert!(limiter.try_consume(a).await);
        assert!(limiter.try_consume(a).await);
        assert!(!limiter.try_consume(a).await); // a is blocked
        assert!(limiter.try_consume(b).await); // b still has budget
    }

    #[tokio::test(start_paused = true)]
    async fn rate_limiter_resets_after_window() {
        let limiter = LoginRateLimiter::new(2, Duration::from_secs(60));
        let ip: IpAddr = "1.2.3.4".parse().unwrap();
        assert!(limiter.try_consume(ip).await);
        assert!(limiter.try_consume(ip).await);
        assert!(!limiter.try_consume(ip).await);
        tokio::time::advance(Duration::from_secs(61)).await;
        assert!(
            limiter.try_consume(ip).await,
            "window should reset after window elapses"
        );
    }

    // ─── BasicAuthCache ───────────────────────────────────────────────────

    #[tokio::test(start_paused = true)]
    async fn basic_auth_cache_hit_then_expire() {
        let cache = BasicAuthCache::new(Duration::from_secs(60));
        cache.insert("deadbeef").await;
        assert!(cache.is_valid("deadbeef").await);
        tokio::time::advance(Duration::from_secs(61)).await;
        assert!(!cache.is_valid("deadbeef").await, "entry should be expired");
    }

    #[tokio::test(start_paused = true)]
    async fn basic_auth_cache_unknown_key_miss() {
        let cache = BasicAuthCache::new(Duration::from_secs(60));
        assert!(!cache.is_valid("never-inserted").await);
    }

    #[tokio::test(start_paused = true)]
    async fn basic_auth_cache_invalidate_all_blows_cache() {
        let cache = BasicAuthCache::new(Duration::from_secs(60));
        cache.insert("a").await;
        cache.insert("b").await;
        assert!(cache.is_valid("a").await);
        cache.invalidate_all().await;
        assert!(!cache.is_valid("a").await);
        assert!(!cache.is_valid("b").await);
    }

    // ─── DB-backed JWT authentication ─────────────────────────────────────

    fn test_state() -> SharedState {
        let db = Arc::new(crate::db::Database::new(":memory:").unwrap());
        let repo = Arc::new(Repository::new(db));
        let (state, _) = ManagerState::new(Settings::default(), repo);
        Arc::new(state)
    }

    fn mint_token(username: &str, role: &str) -> String {
        let claims = crate::domain::Claims {
            sub: username.to_string(),
            role: role.to_string(),
            exp: (chrono::Utc::now() + chrono::Duration::hours(1)).timestamp() as usize,
            ver: 0,
        };
        jsonwebtoken::encode(
            &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),
            &claims,
            &jsonwebtoken::EncodingKey::from_secret(crate::domain::jwt_secret()),
        )
        .unwrap()
    }

    fn make_user(username: &str, role: crate::domain::Role) -> crate::domain::User {
        crate::domain::User {
            id: uuid::Uuid::new_v4().to_string(),
            username: username.to_string(),
            password_hash: "x".to_string(),
            role,
            created_at: chrono::Utc::now(),
            token_version: 0,
        }
    }

    #[tokio::test]
    async fn authenticate_accepts_live_user() {
        let state = test_state();
        state
            .repo
            .insert_user(&make_user("alice", crate::domain::Role::Admin))
            .unwrap();
        let token = mint_token("alice", "ADMIN");
        let user = state
            .authenticate(&token)
            .await
            .expect("live user should authenticate");
        assert_eq!(user.username, "alice");
        assert!(state.authenticate_admin(&token).await.is_ok());
    }

    #[tokio::test]
    async fn authenticate_rejects_token_for_missing_user() {
        // DB wipe / deleted user: validly signed token, but no such user in the DB.
        let state = test_state();
        let token = mint_token("ghost", "ADMIN");
        assert!(state.authenticate(&token).await.is_err());
        assert!(state.authenticate_admin(&token).await.is_err());
    }

    #[tokio::test]
    async fn authenticate_admin_uses_db_role_not_token_role() {
        let state = test_state();
        state
            .repo
            .insert_user(&make_user("bob", crate::domain::Role::User))
            .unwrap();
        // Token forges role ADMIN, but the DB record is USER → the DB wins.
        let token = mint_token("bob", "ADMIN");
        assert!(state.authenticate(&token).await.is_ok());
        assert_eq!(
            state.authenticate_admin(&token).await.unwrap_err(),
            "Admin access required"
        );
    }

    #[tokio::test]
    async fn authenticate_rejects_garbage_token() {
        let state = test_state();
        assert!(state.authenticate("not-a-jwt").await.is_err());
        assert!(state.authenticate("").await.is_err());
    }

    #[tokio::test]
    async fn authenticate_rejects_old_jwt_after_password_change() {
        // After a password change, token_version is bumped in the DB. A JWT
        // minted with the old version must be rejected even though its
        // signature and expiry are still valid.
        let state = test_state();
        state
            .repo
            .insert_user(&make_user("alice", crate::domain::Role::Admin))
            .unwrap();
        let old_token = mint_token("alice", "ADMIN");
        assert!(state.authenticate(&old_token).await.is_ok());
        let new_version = state
            .repo
            .update_user_password_and_bump_version("alice", "newhash")
            .unwrap();
        assert_eq!(new_version, 1);
        assert!(state.authenticate(&old_token).await.is_err());
    }

    // ─── API keys ─────────────────────────────────────────────────────────

    fn insert_key_for(state: &SharedState, user_id: &str) -> String {
        let key = crate::domain::generate_api_key();
        let hash = crate::domain::hash_api_key(&key);
        let prefix = key.chars().take(12).collect::<String>();
        state
            .repo
            .insert_api_key_if_under_cap(
                &crate::db::repository::NewApiKey {
                    id: uuid::Uuid::new_v4().to_string(),
                    user_id: user_id.to_string(),
                    name: "test".to_string(),
                    key_hash: hash,
                    prefix,
                    created_at: chrono::Utc::now().to_rfc3339(),
                },
                1000,
            )
            .unwrap();
        key
    }

    #[tokio::test]
    async fn authenticate_accepts_valid_api_key() {
        let state = test_state();
        let user = make_user("carol", crate::domain::Role::User);
        state.repo.insert_user(&user).unwrap();
        let key = insert_key_for(&state, &user.id);

        let got = state.authenticate(&key).await.expect("api key should auth");
        assert_eq!(got.username, "carol");
    }

    #[tokio::test]
    async fn authenticate_rejects_unknown_or_revoked_api_key() {
        let state = test_state();
        let user = make_user("dave", crate::domain::Role::User);
        state.repo.insert_user(&user).unwrap();
        let key = insert_key_for(&state, &user.id);

        // A well-formed but unknown key.
        assert!(state
            .authenticate(&crate::domain::generate_api_key())
            .await
            .is_err());

        // Revoke the real key → it stops authenticating immediately.
        let keys = state.repo.list_api_keys_for_user(&user.id).unwrap();
        assert_eq!(keys.len(), 1);
        state.repo.delete_api_key(&keys[0].id, &user.id).unwrap();
        assert!(state.authenticate(&key).await.is_err());
    }

    #[tokio::test]
    async fn session_auth_rejects_api_keys_but_accepts_jwt() {
        let state = test_state();
        let user = make_user("erin", crate::domain::Role::Admin);
        state.repo.insert_user(&user).unwrap();
        let key = insert_key_for(&state, &user.id);
        let jwt = mint_token("erin", "ADMIN");

        // API key works on the service surface but NOT on session-only endpoints.
        assert!(state.authenticate(&key).await.is_ok());
        assert!(state.authenticate_session(&key).await.is_err());
        assert!(state.authenticate_session_admin(&key).await.is_err());

        // A real session JWT is accepted by the session paths.
        assert!(state.authenticate_session(&jwt).await.is_ok());
        assert!(state.authenticate_session_admin(&jwt).await.is_ok());
    }

    #[tokio::test]
    async fn api_keys_are_scoped_per_user() {
        let state = test_state();
        let a = make_user("ava", crate::domain::Role::User);
        let b = make_user("ben", crate::domain::Role::User);
        state.repo.insert_user(&a).unwrap();
        state.repo.insert_user(&b).unwrap();
        insert_key_for(&state, &a.id);
        let b_key_id = {
            let key = crate::domain::generate_api_key();
            let hash = crate::domain::hash_api_key(&key);
            let id = uuid::Uuid::new_v4().to_string();
            state
                .repo
                .insert_api_key_if_under_cap(
                    &crate::db::repository::NewApiKey {
                        id: id.clone(),
                        user_id: b.id.clone(),
                        name: "b".to_string(),
                        key_hash: hash,
                        prefix: "p".to_string(),
                        created_at: chrono::Utc::now().to_rfc3339(),
                    },
                    1000,
                )
                .unwrap();
            id
        };

        assert_eq!(state.repo.list_api_keys_for_user(&a.id).unwrap().len(), 1);
        // ava cannot delete ben's key.
        assert!(!state.repo.delete_api_key(&b_key_id, &a.id).unwrap());
        // ben can.
        assert!(state.repo.delete_api_key(&b_key_id, &b.id).unwrap());
    }
}
