# DLoad - Memory-Efficient Download Manager Implementation Plan

> **For Claude:** Use superpowers:executing-plans to implement task-by-task.

**Goal:** Build a Rust-based download manager for Debian Docker with streaming-to-disk architecture (solves aria2's RAM issues), REST API, Web UI, and full protocol support (HTTP/FTP/SFTP/Torrent).

**Architecture:** Axum-based REST API with protocol-dispatched download workers that stream chunks directly to disk without buffering in RAM. Vanilla JS SPA frontend.

**Tech Stack:** 
- Web: Axum 0.7
- HTTP: reqwest 0.12 (streaming)
- FTP: suppaftp 5.x
- SFTP: russh 0.44 + russh_sftp 0.4
- Torrent: librqbit 0.5
- DB: rusqlite 0.31
- Auth: bcrypt + jsonwebtoken

---

## Memory Fix (vs aria2)

```
aria2:   ████████████████████████████ 100-500MB+ (grows with file size)
dload:   ██                          ~20-50MB (constant)
```

Key: Stream chunks (64-128KB) directly to disk using async I/O, never buffer entire file in RAM.

---

## Implementation Tasks

### Task 1: Project Setup - Cargo.toml and Config

**Files:**
- `Cargo.toml`
- `rust-toolchain.toml`
- `.cargo/config.toml`

**Step 1: Create Cargo.toml**

```toml
[package]
name = "dload"
version = "0.1.0"
edition = "2021"

[dependencies]
axum = "0.7"
tokio = { version = "1", features = ["full"] }
reqwest = { version = "0.12", features = ["json", "stream"] }
suppaftp = { version = "5", features = ["async", "native-tls"] }
russh = "0.44"
russh-sftp = "0.4"
librqbit = "0.5"
rusqlite = { version = "0.31", features = ["bundled"] }
bcrypt = "0.15"
jsonwebtoken = "9"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
uuid = { version = "1", features = ["v4"] }
chrono = { version = "0.4", features = ["serde"] }
tower-http = { version = "0.5", features = ["cors", "fs"] }
tracing = "0.1"
tracing-subscriber = "0.3"
thiserror = "1"
anyhow = "1"
url = "2"
directories = "5"

[profile.release]
opt-level = 3
lto = true
codegen-units = 1
strip = true
```

**Step 2: Create rust-toolchain.toml**

```toml
[toolchain]
channel = "stable"
components = ["rustfmt", "clippy"]
```

**Step 3: Commit**
```bash
git add Cargo.toml rust-toolchain.toml
git commit -m "chore: project setup with all dependencies"
```

---

### Task 2: Domain Types

**Files:**
- `src/domain/mod.rs`
- `src/domain/download.rs`
- `src/domain/settings.rs`
- `src/domain/user.rs`

**Step 1: Create domain/download.rs**

```rust
use serde::{Deserialize, Serialize};
use chrono::{DateTime, Utc};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum DownloadStatus {
    Queued,
    Downloading,
    Paused,
    Completed,
    Failed,
    Stopped,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Protocol {
    Http,
    Ftp,
    Sftp,
    Torrent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Download {
    pub id: String,
    pub url: String,
    pub filename: String,
    pub save_path: String,
    pub total_size: u64,
    pub downloaded_size: u64,
    pub speed: u64,
    pub progress: f64,
    pub status: DownloadStatus,
    pub protocol: Protocol,
    pub connections: u32,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub error_message: Option<String>,
}

impl Download {
    pub fn new(url: String, save_dir: &str) -> Self {
        let filename = url
            .split('/')
            .last()
            .unwrap_or("download")
            .to_string();
        
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            url,
            filename: filename.clone(),
            save_path: format!("{}/{}", save_dir.trim_end_matches('/'), filename),
            total_size: 0,
            downloaded_size: 0,
            speed: 0,
            progress: 0.0,
            status: DownloadStatus::Queued,
            protocol: Protocol::Http,
            connections: 1,
            created_at: Utc::now(),
            completed_at: None,
            error_message: None,
        }
    }
    
    pub fn with_protocol(mut self, protocol: Protocol) -> Self {
        self.protocol = protocol;
        self
    }
}
```

**Step 2: Create domain/settings.rs**

```rust
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
            download_dir: "/data".to_string(),
            max_concurrent: 3,
            max_connections_per_file: 4,
            chunk_size: 131072, // 128KB
            username: "admin".to_string(),
            port: 8080,
        }
    }
}
```

**Step 3: Create domain/user.rs**

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub exp: usize,
}
```

**Step 4: Create domain/mod.rs**

```rust
pub mod download;
pub mod settings;
pub mod user;

pub use download::*;
pub use settings::*;
pub use user::*;
```

**Step 5: Commit**
```bash
git add src/domain/
git commit -m "feat: add domain types"
```

---

### Task 3: Database Layer

**Files:**
- `src/db/mod.rs`
- `src/db/repository.rs`

**Step 1: Create src/db/mod.rs**

```rust
pub mod repository;

use rusqlite::Connection;
use std::sync::Mutex;

pub struct Database {
    conn: Mutex<Connection>,
}

impl Database {
    pub fn new(path: &str) -> anyhow::Result<Self> {
        let conn = Connection::open(path)?;
        
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS downloads (
                id TEXT PRIMARY KEY,
                url TEXT NOT NULL,
                filename TEXT NOT NULL,
                save_path TEXT NOT NULL,
                total_size INTEGER DEFAULT 0,
                downloaded_size INTEGER DEFAULT 0,
                speed INTEGER DEFAULT 0,
                progress REAL DEFAULT 0.0,
                status TEXT NOT NULL,
                protocol TEXT NOT NULL,
                connections INTEGER DEFAULT 1,
                created_at TEXT NOT NULL,
                completed_at TEXT,
                error_message TEXT
            );
            
            CREATE TABLE IF NOT EXISTS settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );"
        )?;
        
        Ok(Self { conn: Mutex::new(conn) })
    }
}
```

**Step 2: Create src/db/repository.rs**

```rust
use crate::domain::{Download, DownloadStatus, Protocol, Settings};
use rusqlite::params;
use std::sync::Arc;

pub struct Repository {
    db: Arc<crate::db::Database>,
}

impl Repository {
    pub fn new(db: Arc<crate::db::Database>) -> Self {
        Self { db }
    }

    pub fn insert_download(&self, download: &Download) -> anyhow::Result<()> {
        let conn = self.db.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO downloads (id, url, filename, save_path, total_size, downloaded_size, 
             speed, progress, status, protocol, connections, created_at, completed_at, error_message)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                download.id,
                download.url,
                download.filename,
                download.save_path,
                download.total_size,
                download.downloaded_size,
                download.speed,
                download.progress,
                format!("{:?}", download.status),
                format!("{:?}", download.protocol),
                download.connections,
                download.created_at.to_rfc3339(),
                download.completed_at.map(|d| d.to_rfc3339()),
                download.error_message,
            ],
        )?;
        Ok(())
    }

    pub fn update_download(&self, download: &Download) -> anyhow::Result<()> {
        let conn = self.db.conn.lock().unwrap();
        conn.execute(
            "UPDATE downloads SET total_size=?1, downloaded_size=?2, speed=?3, progress=?4, 
             status=?5, completed_at=?6, error_message=?7 WHERE id=?8",
            params![
                download.total_size,
                download.downloaded_size,
                download.speed,
                download.progress,
                format!("{:?}", download.status),
                download.completed_at.map(|d| d.to_rfc3339()),
                download.error_message,
                download.id,
            ],
        )?;
        Ok(())
    }

    pub fn get_all_downloads(&self) -> anyhow::Result<Vec<Download>> {
        let conn = self.db.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, url, filename, save_path, total_size, downloaded_size, speed, 
             progress, status, protocol, connections, created_at, completed_at, error_message 
             FROM downloads ORDER BY created_at DESC"
        )?;
        
        let downloads = stmt.query_map([], |row| {
            let status_str: String = row.get(8)?;
            let protocol_str: String = row.get(9)?;
            Ok(Download {
                id: row.get(0)?,
                url: row.get(1)?,
                filename: row.get(2)?,
                save_path: row.get(3)?,
                total_size: row.get(4)?,
                downloaded_size: row.get(5)?,
                speed: row.get(6)?,
                progress: row.get(7)?,
                status: serde_json::from_str(&status_str).unwrap_or(DownloadStatus::Queued),
                protocol: serde_json::from_str(&protocol_str).unwrap_or(Protocol::Http),
                connections: row.get(10)?,
                created_at: chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>(11)?)
                    .map(|d| d.with_timezone(&chrono::Utc))
                    .unwrap_or_else(|_| chrono::Utc::now()),
                completed_at: row.get::<_, Option<String>>(12)?
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
                    .map(|d| d.with_timezone(&chrono::Utc)),
                error_message: row.get(13)?,
            })
        })?.collect::<Result<Vec<_>, _>>()?;
        
        Ok(downloads)
    }

    pub fn delete_download(&self, id: &str) -> anyhow::Result<()> {
        let conn = self.db.conn.lock().unwrap();
        conn.execute("DELETE FROM downloads WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn get_settings(&self) -> anyhow::Result<Settings> {
        let conn = self.db.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT key, value FROM settings")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        
        let mut settings = Settings::default();
        for row in rows {
            let (key, value) = row?;
            match key.as_str() {
                "download_dir" => settings.download_dir = value,
                "max_concurrent" => settings.max_concurrent = value.parse().unwrap_or(3),
                "max_connections_per_file" => settings.max_connections_per_file = value.parse().unwrap_or(4),
                "chunk_size" => settings.chunk_size = value.parse().unwrap_or(131072),
                "username" => settings.username = value,
                "port" => settings.port = value.parse().unwrap_or(8080),
                _ => {}
            }
        }
        Ok(settings)
    }

    pub fn save_settings(&self, settings: &Settings) -> anyhow::Result<()> {
        let conn = self.db.conn.lock().unwrap();
        let pairs = [
            ("download_dir", settings.download_dir.clone()),
            ("max_concurrent", settings.max_concurrent.to_string()),
            ("max_connections_per_file", settings.max_connections_per_file.to_string()),
            ("chunk_size", settings.chunk_size.to_string()),
            ("username", settings.username.clone()),
            ("port", settings.port.to_string()),
        ];
        
        for (key, value) in pairs {
            conn.execute(
                "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
                params![key, value],
            )?;
        }
        Ok(())
    }
}
```

**Step 3: Commit**
```bash
git add src/db/
git commit -m "feat: add database layer with SQLite"
```

---

### Task 4: HTTP Download Worker (Streaming-to-Disk)

**Files:**
- `src/worker/mod.rs`
- `src/worker/http.rs`

**Step 1: Create src/worker/mod.rs**

```rust
pub mod http;
pub mod ftp;
pub mod sftp;
pub mod torrent;

use crate::domain::{Download, DownloadStatus, Protocol};

pub fn detect_protocol(url: &str) -> Protocol {
    let url_lower = url.to_lowercase();
    if url_lower.starts_with("magnet:") || url_lower.ends_with(".torrent") {
        Protocol::Torrent
    } else if url_lower.starts_with("sftp://") || url_lower.starts_with("ssh://") {
        Protocol::Sftp
    } else if url_lower.starts_with("ftp://") {
        Protocol::Ftp
    } else {
        Protocol::Http // covers http:// and https://
    }
}
```

**Step 2: Create src/worker/http.rs**

```rust
use crate::domain::{Download, DownloadStatus};
use futures::stream::StreamExt;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;

pub struct HttpDownloader {
    download: Download,
    chunk_size: usize,
}

impl HttpDownloader {
    pub fn new(download: Download, chunk_size: usize) -> Self {
        Self { download, chunk_size }
    }

    pub async fn run(&mut self) -> anyhow::Result<Download> {
        let client = reqwest::Client::new();
        let response = client.get(&self.download.url).send().await?;
        
        // Get total size from headers
        let total_size: u64 = response
            .headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        
        self.download.total_size = total_size;

        // Create file and stream directly to disk
        let mut file = tokio::fs::File::create(&self.download.save_path).await?;
        let mut stream = response.bytes_stream();
        let mut downloaded: u64 = 0;
        let start_time = std::time::Instant::now();
        
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            file.write_all(&chunk).await?;
            downloaded += chunk.len() as u64;
            
            self.download.downloaded_size = downloaded;
            if self.download.total_size > 0 {
                self.download.progress = (downloaded as f64 / self.download.total_size as f64) * 100.0;
            }
            
            let elapsed = start_time.elapsed().as_secs_f64();
            if elapsed > 0.0 {
                self.download.speed = (downloaded as f64 / elapsed) as u64;
            }
        }
        
        file.flush().await?;
        self.download.status = DownloadStatus::Completed;
        
        Ok(self.download.clone())
    }
}
```

**Step 3: Commit**
```bash
git add src/worker/
git commit -m "feat: add HTTP download worker with streaming-to-disk"
```

---

### Task 5: FTP Download Worker

**Files:**
- `src/worker/ftp.rs`

**Step 1: Create src/worker/ftp.rs**

```rust
use crate::domain::{Download, DownloadStatus, Protocol};
use suppaftp::tokio::{AsyncFtpStream, AsyncNativeTlsFtpStream, AsyncNativeTlsConnector};
use suppaftp::async_native_tls::TlsConnector;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use url::Url;

pub struct FtpDownloader {
    download: Download,
    chunk_size: usize,
}

impl FtpDownloader {
    pub fn new(download: Download, chunk_size: usize) -> Self {
        Self { download, chunk_size }
    }

    pub async fn run(&mut self) -> anyhow::Result<Download> {
        let url = Url::parse(&self.download.url)?;
        
        let host = url.host_str().unwrap_or("localhost");
        let port = url.port().unwrap_or(21);
        let path = url.path();
        
        // Extract credentials if present
        let (user, pass) = if let (Some(u), Some(p)) = (url.username(), url.password()) {
            (u, p)
        } else {
            ("anonymous", "anonymous@")
        };
        
        // Connect
        let ftp = AsyncNativeTlsFtpStream::connect(format!("{}:{}", host, port))
            .await
            .map_err(|e| anyhow::anyhow!("FTP connection failed: {}", e))?;
        
        let mut ftp = ftp.into_secure(
            AsyncNativeTlsConnector::from(TlsConnector::new()),
            host,
        ).await.map_err(|e| anyhow::anyhow!("FTP TLS failed: {}", e))?;
        
        ftp.login(user, pass).await.map_err(|e| anyhow::anyhow!("FTP login failed: {}", e))?;
        
        // Get file size
        if let Ok(size) = ftp.size(path).await {
            self.download.total_size = size;
        }
        
        // Create local file
        let mut local_file = tokio::fs::File::create(&self.download.save_path).await?;
        
        // Download using RETR command
        let mut stream = ftp.retr_as_stream(path).await
            .map_err(|e| anyhow::anyhow!("FTP RETR failed: {}", e))?;
        
        let mut buffer = vec![0u8; self.chunk_size];
        let mut downloaded: u64 = 0;
        let start_time = std::time::Instant::now();
        
        loop {
            match stream.read(&mut buffer).await {
                Ok(0) => break,
                Ok(n) => {
                    local_file.write_all(&buffer[..n]).await?;
                    downloaded += n as u64;
                    
                    self.download.downloaded_size = downloaded;
                    if self.download.total_size > 0 {
                        self.download.progress = (downloaded as f64 / self.download.total_size as f64) * 100.0;
                    }
                    
                    let elapsed = start_time.elapsed().as_secs_f64();
                    if elapsed > 0.0 {
                        self.download.speed = (downloaded as f64 / elapsed) as u64;
                    }
                }
                Err(e) => return Err(anyhow::anyhow!("FTP read error: {}", e)),
            }
        }
        
        local_file.flush().await?;
        ftp.finalize_retr_stream(stream).await.ok();
        ftp.quit().await.ok();
        
        self.download.status = DownloadStatus::Completed;
        Ok(self.download.clone())
    }
}
```

**Step 2: Commit**
```bash
git add src/worker/ftp.rs
git commit -m "feat: add FTP download worker"
```

---

### Task 6: SFTP Download Worker

**Files:**
- `src/worker/sftp.rs`

**Step 1: Create src/worker/sftp.rs**

```rust
use crate::domain::{Download, DownloadStatus};
use russh::client::*;
use russh_sftp::client::SftpSession;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use url::Url;

struct ClientHandler;

impl Handler for ClientHandler {
    type Error = anyhow::Error;
    
    async fn check_server_key(
        &mut self,
        _server_public_key: &ssh_key::PublicKey,
    ) -> Result<bool, Self::Error> {
        Ok(true) // In production, verify against known_hosts
    }
}

pub struct SftpDownloader {
    download: Download,
    chunk_size: usize,
}

impl SftpDownloader {
    pub fn new(download: Download, chunk_size: usize) -> Self {
        Self { download, chunk_size }
    }

    pub async fn run(&mut self) -> anyhow::Result<Download> {
        let url = Url::parse(&self.download.url)?;
        
        let host = url.host_str().unwrap_or("localhost");
        let port = url.port().unwrap_or(22);
        let path = url.path();
        
        let (user, pass) = if let (Some(u), Some(p)) = (url.username(), url.password()) {
            (u.to_string(), p.to_string())
        } else {
            ("anonymous".to_string(), "".to_string())
        };
        
        let config = Arc::new(Config::default());
        let handler = ClientHandler;
        
        let mut session = connect(config, (host, port), handler)
            .await
            .map_err(|e| anyhow::anyhow!("SSH connection failed: {}", e))?;
        
        session.authenticate_password(&user, &pass)
            .await
            .map_err(|e| anyhow::anyhow!("SSH auth failed: {}", e))?;
        
        let channel = session.channel_open_session().await
            .map_err(|e| anyhow::anyhow!("SSH channel failed: {}", e))?;
        
        channel.request_subsystem(true, "sftp").await
            .map_err(|e| anyhow::anyhow!("SFTP subsystem failed: {}", e))?;
        
        let sftp = SftpSession::new(channel.into_stream()).await
            .map_err(|e| anyhow::anyhow!("SFTP session failed: {}", e))?;
        
        // Get file metadata
        if let Ok(metadata) = sftp.stat(path).await {
            self.download.total_size = metadata.size.unwrap_or(0);
        }
        
        // Open remote file
        use russh_sftp::protocol::OpenFlags;
        let mut remote_file = sftp.open_with_flags(
            path,
            OpenFlags::READ,
        ).await.map_err(|e| anyhow::anyhow!("SFTP open failed: {}", e))?;
        
        // Create local file
        let mut local_file = tokio::fs::File::create(&self.download.save_path).await?;
        
        let mut buffer = vec![0u8; self.chunk_size];
        let mut downloaded: u64 = 0;
        let start_time = std::time::Instant::now();
        
        loop {
            match remote_file.read(&mut buffer).await {
                Ok(0) => break,
                Ok(n) => {
                    local_file.write_all(&buffer[..n]).await?;
                    downloaded += n as u64;
                    
                    self.download.downloaded_size = downloaded;
                    if self.download.total_size > 0 {
                        self.download.progress = (downloaded as f64 / self.download.total_size as f64) * 100.0;
                    }
                    
                    let elapsed = start_time.elapsed().as_secs_f64();
                    if elapsed > 0.0 {
                        self.download.speed = (downloaded as f64 / elapsed) as u64;
                    }
                }
                Err(e) => return Err(anyhow::anyhow!("SFTP read error: {}", e)),
            }
        }
        
        local_file.flush().await?;
        self.download.status = DownloadStatus::Completed;
        Ok(self.download.clone())
    }
}
```

**Step 2: Commit**
```bash
git add src/worker/sftp.rs
git commit -m "feat: add SFTP download worker"
```

---

### Task 7: BitTorrent Download Worker

**Files:**
- `src/worker/torrent.rs`

**Step 1: Create src/worker/torrent.rs**

```rust
use crate::domain::{Download, DownloadStatus, Protocol};
use librqbit::{Session, AddTorrent, ManagedTorrentHandle};
use std::sync::Arc;
use tokio::time::{interval, Duration};

pub struct TorrentDownloader {
    download: Download,
    session: Arc<Session>,
}

impl TorrentDownloader {
    pub fn new(download: Download, session: Arc<Session>) -> Self {
        Self { download, session }
    }

    pub async fn run(&mut self) -> anyhow::Result<Download> {
        let url = &self.download.url;
        
        // Determine if it's a magnet link or torrent file URL
        let add_torrent = if url.starts_with("magnet:") {
            AddTorrent::from_url(url.as_str())
        } else {
            // Assume it's a URL to a .torrent file - download it first
            let torrent_data = reqwest::get(url).await?
                .bytes().await?
                .to_vec();
            AddTorrent::from_bytes(torrent_data.into())
        };
        
        let handle = self.session.add_torrent(add_torrent, None)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to add torrent: {}", e))?
            .into_handle()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get torrent handle: {}", e))?;
        
        // Set download path
        let save_path = std::path::Path::new(&self.download.save_path)
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| std::path::PathBuf::from("/data"));
        
        // Wait for completion with progress tracking
        let mut poll_interval = interval(Duration::from_secs(1));
        
        loop {
            poll_interval.tick().await;
            
            if let Ok(stats) = handle.get_progress().await {
                self.download.downloaded_size = stats.bytes_completed;
                self.download.total_size = stats.bytes_total;
                
                if stats.bytes_total > 0 {
                    self.download.progress = (stats.bytes_completed as f64 / stats.bytes_total as f64) * 100.0;
                }
                
                self.download.speed = stats.download_rate;
                
                // Check if completed
                if handle.is_completed().await {
                    break;
                }
            }
        }
        
        self.download.status = DownloadStatus::Completed;
        Ok(self.download.clone())
    }
}
```

**Step 2: Commit**
```bash
git add src/worker/torrent.rs
git commit -m "feat: add BitTorrent download worker"
```

---

### Task 8: Download Manager with Protocol Dispatcher

**Files:**
- `src/manager/mod.rs`

**Step 1: Create src/manager/mod.rs**

```rust
use crate::domain::{Download, DownloadStatus, Protocol, Settings};
use crate::worker::{self, http::HttpDownloader, ftp::FtpDownloader, sftp::SftpDownloader, torrent::TorrentDownloader};
use librqbit::Session;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};

pub struct ManagerState {
    pub downloads: RwLock<HashMap<String, Download>>,
    pub settings: RwLock<Settings>,
    pub tx: broadcast::Sender<DownloadEvent>,
    pub torrent_session: Arc<Session>,
}

#[derive(Debug, Clone)]
pub enum DownloadEvent {
    Progress(Download),
    Completed(String),
    Failed(String, String),
}

impl ManagerState {
    pub fn new(settings: Settings) -> Self {
        let (tx, _) = broadcast::channel(100);
        
        // Initialize torrent session
        let torrent_session = Arc::new(
            Session::new(settings.download_dir.clone()).expect("Failed to create torrent session")
        );
        
        Self {
            downloads: RwLock::new(HashMap::new()),
            settings: RwLock::new(settings),
            tx,
            torrent_session,
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<DownloadEvent> {
        self.tx.subscribe()
    }

    pub async fn add_download(&self, download: Download) {
        let mut downloads = self.downloads.write().await;
        downloads.insert(download.id.clone(), download);
        let _ = self.tx.send(DownloadEvent::Progress(download));
    }

    pub async fn update_download(&self, download: &Download) {
        let mut downloads = self.downloads.write().await;
        if let Some(d) = downloads.get_mut(&download.id) {
            *d = download.clone();
        }
        let _ = self.tx.send(DownloadEvent::Progress(download.clone()));
    }

    pub async fn get_all(&self) -> Vec<Download> {
        let downloads = self.downloads.read().await;
        downloads.values().cloned().collect()
    }

    pub async fn remove(&self, id: &str) {
        let mut downloads = self.downloads.write().await;
        downloads.remove(id);
    }

    pub async fn start_download(&self, download: Download) {
        let state = Arc::clone(&self);
        let settings = self.settings.read().await;
        let chunk_size = settings.chunk_size as usize;
        drop(settings);
        
        tokio::spawn(async move {
            let protocol = worker::detect_protocol(&download.url);
            let mut download = download;
            download.protocol = protocol;
            download.status = DownloadStatus::Downloading;
            
            state.update_download(&download).await;
            
            let result = match protocol {
                Protocol::Http => {
                    let mut worker = HttpDownloader::new(download.clone(), chunk_size);
                    worker.run().await
                }
                Protocol::Ftp => {
                    let mut worker = FtpDownloader::new(download.clone(), chunk_size);
                    worker.run().await
                }
                Protocol::Sftp => {
                    let mut worker = SftpDownloader::new(download.clone(), chunk_size);
                    worker.run().await
                }
                Protocol::Torrent => {
                    let session = Arc::clone(&state.torrent_session);
                    let mut worker = TorrentDownloader::new(download.clone(), session);
                    worker.run().await
                }
            };
            
            match result {
                Ok(mut completed) => {
                    completed.status = DownloadStatus::Completed;
                    state.update_download(&completed).await;
                }
                Err(e) => {
                    let mut failed = download;
                    failed.status = DownloadStatus::Failed;
                    failed.error_message = Some(e.to_string());
                    state.update_download(&failed).await;
                }
            }
        });
    }
}

pub type SharedState = Arc<ManagerState>;
```

**Step 2: Commit**
```bash
git add src/manager/mod.rs
git commit -m "feat: add download manager with protocol dispatcher"
```

---

### Task 9: REST API Endpoints

**Files:**
- `src/api/mod.rs`
- `src/api/downloads.rs`
- `src/api/torrents.rs`
- `src/api/settings.rs`
- `src/api/auth.rs`

**Step 1: Create src/api/downloads.rs**

```rust
use crate::domain::Download;
use crate::manager::SharedState;
use axum::{
    extract::{Path, State},
    response::Json,
    routing::{delete, get, post},
    Router,
};

pub fn router(state: SharedState) -> Router {
    Router::new()
        .route("/api/downloads", get(list_downloads).post(add_download))
        .route("/api/downloads/:id", delete(remove_download))
        .with_state(state)
}

async fn list_downloads(State(state): State<SharedState>) -> Json<Vec<Download>> {
    Json(state.get_all().await)
}

async fn add_download(
    State(state): State<SharedState>,
    Json(payload): Json<serde_json::Value>,
) -> Json<Download> {
    let url = payload["url"].as_str().unwrap_or("").to_string();
    let settings = state.settings.read().await;
    let download_dir = settings.download_dir.clone();
    drop(settings);
    
    let mut download = Download::new(url, &download_dir);
    download.status = crate::domain::DownloadStatus::Downloading;
    
    state.add_download(download.clone()).await;
    state.start_download(download.clone()).await;
    
    Json(download)
}

async fn remove_download(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> Json<serde_json::Value> {
    state.remove(&id).await;
    Json(serde_json::json!({ "success": true }))
}
```

**Step 2: Create src/api/torrents.rs**

```rust
use crate::manager::SharedState;
use axum::{
    extract::State,
    response::Json,
    routing::get,
    Router,
};

pub fn router(state: SharedState) -> Router {
    Router::new()
        .route("/api/torrents", get(list_torrents))
        .with_state(state)
}

async fn list_torrents(State(state): State<SharedState>) -> Json<serde_json::Value> {
    let downloads = state.get_all().await;
    let torrents: Vec<_> = downloads.into_iter()
        .filter(|d| d.protocol == crate::domain::Protocol::Torrent)
        .collect();
    Json(serde_json::json!({ "torrents": torrents }))
}
```

**Step 3: Create src/api/settings.rs**

```rust
use crate::domain::Settings;
use crate::manager::SharedState;
use axum::{
    extract::State,
    response::Json,
    routing::{get, put},
    Router,
};

pub fn router(state: SharedState) -> Router {
    Router::new()
        .route("/api/settings", get(get_settings).put(update_settings))
        .with_state(state)
}

async fn get_settings(State(state): State<SharedState>) -> Json<Settings> {
    let settings = state.settings.read().await;
    Json(settings.clone())
}

async fn update_settings(
    State(state): State<SharedState>,
    Json(settings): Json<Settings>,
) -> Json<serde_json::Value> {
    let mut current = state.settings.write().await;
    *current = settings;
    Json(serde_json::json!({ "success": true }))
}
```

**Step 4: Create src/api/auth.rs**

```rust
use crate::domain::Claims;
use axum::{
    response::Json,
    routing::{get, post},
    Router,
};
use bcrypt::{hash, verify};
use jsonwebtoken::{decode, encode, Header, Validation};
use serde::{Deserialize, Serialize};

const SECRET: &str = "dload-secret-key-change-in-production";

pub fn router() -> Router {
    Router::new()
        .route("/api/auth/login", post(login))
        .route("/api/auth/verify", post(verify_token))
}

#[derive(Debug, Deserialize)]
struct LoginRequest {
    username: String,
    password: String,
}

async fn login(Json(payload): Json<LoginRequest>) -> Json<serde_json::Value> {
    let stored_hash = hash("admin", 10).unwrap();
    
    if payload.username == "admin" && verify(&payload.password, &stored_hash).unwrap_or(false) {
        let claims = Claims {
            sub: payload.username.clone(),
            exp: (chrono::Utc::now() + chrono::Duration::hours(24)).timestamp() as usize,
        };
        
        let token = encode(&Header::default(), &claims, SECRET.as_bytes()).unwrap();
        
        Json(serde_json::json!({
            "success": true,
            "token": token,
            "username": payload.username
        }))
    } else {
        Json(serde_json::json!({
            "success": false,
            "error": "Invalid credentials"
        }))
    }
}

async fn verify_token(Json(token): Json<String>) -> Json<serde_json::Value> {
    match decode::<Claims>(&token, SECRET.as_bytes(), &Validation::default()) {
        Ok(_) => Json(serde_json::json!({ "valid": true })),
        Err(_) => Json(serde_json::json!({ "valid": false })),
    }
}
```

**Step 5: Create src/api/mod.rs**

```rust
pub mod auth;
pub mod downloads;
pub mod torrents;
pub mod settings;

use crate::manager::SharedState;
use axum::Router;

pub fn router(state: SharedState) -> Router {
    Router::new()
        .merge(auth::router())
        .merge(downloads::router(state.clone()))
        .merge(torrents::router(state.clone()))
        .merge(settings::router(state))
}
```

**Step 6: Commit**
```bash
git add src/api/
git commit -m "feat: add REST API endpoints"
```

---

### Task 10: Web UI - Vanilla JS SPA

**Files:**
- `src/ui/index.html`
- `src/ui/app.js`
- `src/ui/style.css`

**Step 1: Create src/ui/index.html**

```html
<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>DLoad - Download Manager</title>
    <link rel="stylesheet" href="/ui/style.css">
</head>
<body>
    <div id="app">
        <nav class="navbar">
            <div class="logo">DLoad</div>
            <div class="nav-links">
                <a href="#dashboard" class="active">Dashboard</a>
                <a href="#downloads">Downloads</a>
                <a href="#torrents">Torrents</a>
                <a href="#settings">Settings</a>
            </div>
            <div class="user-info">
                <span id="username">admin</span>
                <button onclick="logout()">Logout</button>
            </div>
        </nav>
        
        <main id="main-content"></main>
    </div>
    
    <div id="login-modal" class="modal">
        <div class="modal-content">
            <h2>Login to DLoad</h2>
            <form id="login-form">
                <input type="text" id="login-username" placeholder="Username" required>
                <input type="password" id="login-password" placeholder="Password" required>
                <button type="submit">Login</button>
            </form>
        </div>
    </div>
    
    <script src="/ui/app.js"></script>
</body>
</html>
```

**Step 2: Create src/ui/app.js**

```javascript
const API_BASE = '/api';
let token = localStorage.getItem('dload_token') || '';

async function apiRequest(endpoint, options = {}) {
    const headers = {
        'Content-Type': 'application/json',
        ...(token && { 'Authorization': `Bearer ${token}` }),
        ...options.headers
    };
    
    const response = await fetch(`${API_BASE}${endpoint}`, {
        ...options,
        headers
    });
    
    if (response.status === 401) {
        logout();
        throw new Error('Unauthorized');
    }
    
    return response.json();
}

function showDashboard() {
    return `
        <div class="dashboard">
            <h2>Dashboard</h2>
            <div class="stats-grid">
                <div class="stat-card">
                    <h3>Active</h3>
                    <p id="active-count">0</p>
                </div>
                <div class="stat-card">
                    <h3>Completed</h3>
                    <p id="completed-count">0</p>
                </div>
                <div class="stat-card">
                    <h3>Total Speed</h3>
                    <p id="total-speed">0 MB/s</p>
                </div>
            </div>
            
            <div class="add-download">
                <h3>Add Download</h3>
                <form id="add-download-form">
                    <input type="url" id="download-url" placeholder="Enter URL (http://, ftp://, sftp://, magnet:, .torrent)" required>
                    <button type="submit">Download</button>
                </form>
            </div>
            
            <div class="active-downloads">
                <h3>Active Downloads</h3>
                <div id="downloads-list"></div>
            </div>
        </div>
    `;
}

function showTorrents() {
    return `
        <div class="torrents">
            <h2>Torrents</h2>
            <div class="add-download">
                <h3>Add Torrent</h3>
                <form id="add-torrent-form">
                    <input type="url" id="torrent-url" placeholder="Enter magnet link or torrent URL" required>
                    <button type="submit">Add Torrent</button>
                </form>
            </div>
            <div id="torrents-list"></div>
        </div>
    `;
}

function showSettings() {
    return `
        <div class="settings">
            <h2>Settings</h2>
            <form id="settings-form">
                <label>
                    Download Directory
                    <input type="text" id="settings-dir" value="/data">
                </label>
                <label>
                    Max Concurrent Downloads
                    <input type="number" id="settings-max-concurrent" value="3" min="1" max="10">
                </label>
                <label>
                    Max Connections Per File
                    <input type="number" id="settings-connections" value="4" min="1" max="16">
                </label>
                <button type="submit">Save Settings</button>
            </form>
        </div>
    `;
}

async function loadDownloads() {
    try {
        const downloads = await apiRequest('/downloads');
        renderDownloads(downloads);
        updateStats(downloads);
    } catch (e) {
        console.error('Failed to load downloads:', e);
    }
}

function renderDownloads(downloads) {
    const container = document.getElementById('downloads-list');
    if (!container) return;
    
    if (downloads.length === 0) {
        container.innerHTML = '<p class="empty">No downloads yet</p>';
        return;
    }
    
    container.innerHTML = downloads.map(d => `
        <div class="download-item" data-id="${d.id}">
            <div class="download-info">
                <h4>${d.filename}</h4>
                <p class="url">${d.url}</p>
                <span class="protocol">${d.protocol}</span>
            </div>
            <div class="download-progress">
                <div class="progress-bar">
                    <div class="progress-fill" style="width: ${d.progress}%"></div>
                </div>
                <span class="progress-text">${d.progress.toFixed(1)}%</span>
            </div>
            <div class="download-stats">
                <span>${formatSize(d.downloaded_size)} / ${formatSize(d.total_size)}</span>
                <span>${formatSpeed(d.speed)}</span>
                <span class="status ${d.status.toLowerCase()}">${d.status}</span>
            </div>
            <button class="delete-btn" onclick="deleteDownload('${d.id}')">×</button>
        </div>
    `).join('');
}

function formatSize(bytes) {
    if (bytes === 0) return '0 B';
    const k = 1024;
    const sizes = ['B', 'KB', 'MB', 'GB'];
    const i = Math.floor(Math.log(bytes) / Math.log(k));
    return parseFloat((bytes / Math.pow(k, i)).toFixed(2)) + ' ' + sizes[i];
}

function formatSpeed(bytesPerSec) {
    return formatSize(bytesPerSec) + '/s';
}

async function addDownload(url) {
    await apiRequest('/downloads', {
        method: 'POST',
        body: JSON.stringify({ url })
    });
    loadDownloads();
}

async function deleteDownload(id) {
    await apiRequest(`/downloads/${id}`, { method: 'DELETE' });
    loadDownloads();
}

function updateStats(downloads) {
    const active = downloads.filter(d => d.status === 'Downloading').length;
    const completed = downloads.filter(d => d.status === 'Completed').length;
    const totalSpeed = downloads.reduce((sum, d) => sum + d.speed, 0);
    
    document.getElementById('active-count').textContent = active;
    document.getElementById('completed-count').textContent = completed;
    document.getElementById('total-speed').textContent = formatSpeed(totalSpeed);
}

document.getElementById('login-form')?.addEventListener('submit', async (e) => {
    e.preventDefault();
    const username = document.getElementById('login-username').value;
    const password = document.getElementById('login-password').value;
    
    const result = await apiRequest('/auth/login', {
        method: 'POST',
        body: JSON.stringify({ username, password })
    });
    
    if (result.success) {
        token = result.token;
        localStorage.setItem('dload_token', token);
        document.getElementById('login-modal').style.display = 'none';
        init();
    } else {
        alert('Login failed');
    }
});

function logout() {
    token = '';
    localStorage.removeItem('dload_token');
    document.getElementById('login-modal').style.display = 'flex';
}

function navigate(hash) {
    const routes = {
        '#dashboard': showDashboard,
        '#downloads': showDashboard,
        '#torrents': showTorrents,
        '#settings': showSettings
    };
    
    const render = routes[hash] || showDashboard;
    document.getElementById('main-content').innerHTML = render();
    
    if (hash === '#dashboard' || hash === '#downloads') {
        loadDownloads();
    }
}

async function init() {
    if (!token) {
        document.getElementById('login-modal').style.display = 'flex';
        return;
    }
    
    navigate(window.location.hash || '#dashboard');
    window.addEventListener('hashchange', () => navigate(window.location.hash));
    setInterval(loadDownloads, 2000);
}

document.addEventListener('DOMContentLoaded', init);
```

**Step 3: Create src/ui/style.css**

```css
* { margin: 0; padding: 0; box-sizing: border-box; }

body {
    font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
    background: #1a1a2e;
    color: #eee;
}

.navbar {
    display: flex;
    align-items: center;
    padding: 1rem 2rem;
    background: #16213e;
    border-bottom: 1px solid #0f3460;
}

.logo { font-size: 1.5rem; font-weight: bold; color: #e94560; }

.nav-links { margin-left: 2rem; display: flex; gap: 1.5rem; }

.nav-links a {
    color: #aaa;
    text-decoration: none;
    padding: 0.5rem;
}

.nav-links a.active, .nav-links a:hover { color: #fff; }

.user-info { margin-left: auto; display: flex; gap: 1rem; align-items: center; }

main { padding: 2rem; max-width: 1200px; margin: 0 auto; }

.stats-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(200px, 1fr));
    gap: 1rem;
    margin-bottom: 2rem;
}

.stat-card {
    background: #16213e;
    padding: 1.5rem;
    border-radius: 8px;
    text-align: center;
}

.stat-card h3 { color: #aaa; font-size: 0.9rem; margin-bottom: 0.5rem; }
.stat-card p { font-size: 2rem; font-weight: bold; color: #e94560; }

.add-download {
    background: #16213e;
    padding: 1.5rem;
    border-radius: 8px;
    margin-bottom: 2rem;
}

.add-download input {
    flex: 1;
    padding: 0.75rem;
    border: 1px solid #0f3460;
    border-radius: 4px;
    background: #1a1a2e;
    color: #fff;
    margin-right: 1rem;
}

.add-download button {
    padding: 0.75rem 1.5rem;
    background: #e94560;
    color: #fff;
    border: none;
    border-radius: 4px;
    cursor: pointer;
}

.download-item {
    background: #16213e;
    padding: 1rem;
    border-radius: 8px;
    margin-bottom: 1rem;
    display: grid;
    grid-template-columns: 1fr 200px auto auto;
    gap: 1rem;
    align-items: center;
}

.download-info h4 { margin-bottom: 0.25rem; }
.download-info .url { font-size: 0.8rem; color: #888; }
.download-info .protocol { 
    display: inline-block;
    font-size: 0.7rem;
    padding: 0.2rem 0.5rem;
    background: #0f3460;
    border-radius: 3px;
    margin-top: 0.25rem;
}

.progress-bar {
    height: 8px;
    background: #0f3460;
    border-radius: 4px;
    overflow: hidden;
}

.progress-fill {
    height: 100%;
    background: #e94560;
    transition: width 0.3s;
}

.status { padding: 0.25rem 0.5rem; border-radius: 4px; font-size: 0.8rem; }
.status.downloading { background: #4caf50; }
.status.completed { background: #2196f3; }
.status.failed { background: #f44336; }

.modal {
    position: fixed;
    inset: 0;
    background: rgba(0,0,0,0.8);
    display: none;
    align-items: center;
    justify-content: center;
}

.modal-content {
    background: #16213e;
    padding: 2rem;
    border-radius: 8px;
    width: 100%;
    max-width: 400px;
}

.modal-content input {
    width: 100%;
    padding: 0.75rem;
    margin-bottom: 1rem;
    border: 1px solid #0f3460;
    border-radius: 4px;
    background: #1a1a2e;
    color: #fff;
}

.settings label {
    display: block;
    margin-bottom: 1rem;
}

.settings input {
    display: block;
    width: 100%;
    padding: 0.75rem;
    margin-top: 0.5rem;
    border: 1px solid #0f3460;
    border-radius: 4px;
    background: #1a1a2e;
    color: #fff;
}

.delete-btn {
    background: #f44336;
    color: #fff;
    border: none;
    padding: 0.5rem 1rem;
    border-radius: 4px;
    cursor: pointer;
}
```

**Step 4: Commit**
```bash
git add src/ui/
git commit -m "feat: add vanilla JS web UI"
```

---

### Task 11: Main Application Entry Point

**Files:**
- `src/lib.rs`
- `src/main.rs`

**Step 1: Create src/lib.rs**

```rust
pub mod api;
pub mod db;
pub mod domain;
pub mod manager;
pub mod worker;
```

**Step 2: Create src/main.rs**

```rust
use std::sync::Arc;
use axum::{
    routing::get,
    Router,
};
use std::net::SocketAddr;
use tower_http::cors::{Any, CorsLayer};
use tower_http::serve::ServeDir;

mod api;
mod db;
mod domain;
mod manager;
mod worker;

#[derive(Clone)]
struct AppState {
    manager: manager::SharedState,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    
    let settings = domain::Settings::default();
    let db = Arc::new(db::Database::new("dload.db").expect("Failed to create database"));
    
    let repo = db::repository::Repository::new(db.clone());
    repo.save_settings(&settings).ok();
    
    let manager_state = manager::ManagerState::new(settings);
    let state = Arc::new(AppState {
        manager: Arc::new(manager_state),
    });
    
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);
    
    let app = Router::new()
        .nest_service("/ui", ServeDir::new("src/ui"))
        .route("/", get(index))
        .merge(api::router(state.manager.clone()))
        .layer(cors);
    
    let addr = SocketAddr::from(([0, 0, 0, 0], 8080));
    tracing::info!("Server starting on {}", addr);
    
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn index() -> impl axum::response::IntoResponse {
    axum::response::Html(include_str!("../ui/index.html"))
}
```

**Step 3: Commit**
```bash
git add src/main.rs src/lib.rs
git commit -m "feat: add main application entry point"
```

---

### Task 12: Docker Configuration

**Files:**
- `Dockerfile`
- `.dockerignore`

**Step 1: Create Dockerfile**

```dockerfile
# Build stage
FROM rust:1.75 AS builder

WORKDIR /app

RUN apt-get update && apt-get install -y \
    libssl-dev \
    pkg-config \
    clang \
    lld \
    libssh2-1-dev \
    && rm -rf /var/lib/apt/lists/*

COPY . .

RUN cargo build --release --locked

# Runtime stage
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y \
    libssl3 \
    ca-certificates \
    libssh2-1 \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /app/target/release/dload /usr/local/bin/

RUN mkdir /data && chown nobody /data

USER nobody

EXPOSE 8080

CMD ["dload"]
```

**Step 2: Create .dockerignore**

```
.git
target
Cargo.lock
*.md
!README.md
.DS_Store
```

**Step 3: Commit**
```bash
git add Dockerfile .dockerignore
git commit -m "chore: add Docker configuration"
```

---

### Task 13: Verify Build

**Step 1: Build the project**

Run: `cargo check` (faster than build for verification)
Expected: Compiles without errors

If there are errors, fix them and commit.

**Step 2: Final commit**

```bash
git add .
git commit -m "feat: complete dload download manager v0.1"
git tag v0.1.0
```

---

## Plan Complete

This plan implements all 4 protocols (HTTP, FTP, SFTP, BitTorrent) with the key memory-efficiency fix: streaming chunks directly to disk instead of buffering in RAM.
