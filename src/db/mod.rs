pub mod repository;

use rusqlite::Connection;
use std::sync::Mutex;

pub struct Database {
    pub(crate) conn: Mutex<Connection>,
}

impl Database {
    pub fn new(path: &str) -> anyhow::Result<Self> {
        let conn = Connection::open(path)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;

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
            );

            CREATE TABLE IF NOT EXISTS users (
                id TEXT PRIMARY KEY,
                username TEXT NOT NULL UNIQUE,
                password_hash TEXT NOT NULL,
                role TEXT NOT NULL DEFAULT 'USER',
                created_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS download_history (
                id TEXT PRIMARY KEY,
                url TEXT NOT NULL,
                filename TEXT NOT NULL,
                total_size INTEGER DEFAULT 0,
                status TEXT NOT NULL,
                protocol TEXT NOT NULL,
                created_at TEXT NOT NULL,
                completed_at TEXT
            );

            CREATE TABLE IF NOT EXISTS api_keys (
                id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL,
                name TEXT NOT NULL,
                key_hash TEXT NOT NULL UNIQUE,
                prefix TEXT NOT NULL,
                created_at TEXT NOT NULL,
                last_used_at TEXT
            );",
        )?;

        if let Err(e) = conn.execute_batch(
            "ALTER TABLE downloads ADD COLUMN info_hash TEXT;
             ALTER TABLE downloads ADD COLUMN category TEXT;
             ALTER TABLE downloads ADD COLUMN content_path TEXT;",
        ) {
            tracing::warn!("migration info_hash/category/content_path failed: {}", e);
        }
        if let Err(e) = conn.execute_batch(
            "ALTER TABLE downloads ADD COLUMN http_mirror_status TEXT;
             ALTER TABLE downloads ADD COLUMN http_mirror_url TEXT;",
        ) {
            tracing::warn!("migration http_mirror_status/http_mirror_url failed: {}", e);
        }
        let _ = conn
            .execute_batch("ALTER TABLE downloads ADD COLUMN restart_resume INTEGER DEFAULT 0;");
        // One-time migration: existing torrent downloads should auto-resume.
        // Includes Paused because the old code marked active torrents as Paused on shutdown,
        // so there's no way to distinguish user-paused from system-paused on first upgrade.
        // Guarded by a settings flag so it only runs once (not on every restart).
        {
            let already_done: bool = conn
                .query_row(
                    "SELECT COUNT(*) FROM settings WHERE key = 'migration_restart_resume'",
                    [],
                    |row| row.get::<_, i32>(0),
                )
                .unwrap_or(0)
                > 0;
            if !already_done {
                if let Err(e) = conn.execute_batch(
                    "UPDATE downloads SET restart_resume = 1
                     WHERE protocol = 'Torrent'
                     AND status IN ('Downloading', 'Seeding', 'Paused', 'Queued')
                     AND restart_resume = 0;
                     INSERT OR IGNORE INTO settings (key, value) VALUES ('migration_restart_resume', '1');",
                ) {
                    tracing::warn!("migration restart_resume failed: {}", e);
                }
            }
        }
        if let Err(e) = conn.execute_batch(
            "ALTER TABLE downloads ADD COLUMN download_folder TEXT NOT NULL DEFAULT '';",
        ) {
            tracing::warn!("migration download_folder failed: {}", e);
        }
        // One-time migration: backfill download_folder from existing content_path/save_path.
        // Priority per row: parent(content_path) if set → parent(save_path) if non-empty →
        // configured default folder. Rows where pre-fix content_path == save_path end up at the
        // default folder; those self-heal when the torrent next re-enters the session.
        {
            let already_done: bool = conn
                .query_row(
                    "SELECT COUNT(*) FROM settings WHERE key = 'migration_download_folder_backfill'",
                    [],
                    |row| row.get::<_, i32>(0),
                )
                .unwrap_or(0)
                > 0;
            if !already_done {
                let default_dir: String = conn
                    .query_row(
                        "SELECT value FROM settings WHERE key = 'download_dir'",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap_or_else(|_| "/downloads".to_string());

                let rows: Vec<(String, Option<String>, String)> = {
                    let mut stmt = conn.prepare(
                        "SELECT id, content_path, save_path FROM downloads
                         WHERE download_folder = '' OR download_folder IS NULL",
                    )?;
                    let iter = stmt.query_map([], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, Option<String>>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    })?;
                    iter.collect::<Result<Vec<_>, _>>()?
                };

                let parent_of = |p: &str| -> Option<String> {
                    let trimmed = p.trim_end_matches('/');
                    std::path::Path::new(trimmed)
                        .parent()
                        .filter(|p| !p.as_os_str().is_empty())
                        .map(|p| p.to_string_lossy().to_string())
                };

                for (id, content_path, save_path) in rows {
                    let folder = content_path
                        .as_deref()
                        .filter(|s| !s.is_empty())
                        .and_then(parent_of)
                        .or_else(|| parent_of(&save_path))
                        .unwrap_or_else(|| default_dir.clone());
                    if let Err(e) = conn.execute(
                        "UPDATE downloads SET download_folder = ?1 WHERE id = ?2",
                        rusqlite::params![&folder, &id],
                    ) {
                        tracing::warn!(
                            "migration download_folder backfill failed for {}: {}",
                            id,
                            e
                        );
                    }
                }

                if let Err(e) = conn.execute(
                    "INSERT OR IGNORE INTO settings (key, value) VALUES (?1, ?2)",
                    rusqlite::params![&"migration_download_folder_backfill", &"1"],
                ) {
                    tracing::warn!("migration download_folder backfill marker failed: {}", e);
                }
            }
        }

        if let Err(e) =
            conn.execute_batch("ALTER TABLE downloads ADD COLUMN position INTEGER DEFAULT 0;")
        {
            tracing::warn!("migration position failed: {}", e);
        }
        // One-time migration: backfill positions from created_at order so existing
        // downloads get a sensible initial ordering.
        {
            let already_done: bool = conn
                .query_row(
                    "SELECT COUNT(*) FROM settings WHERE key = 'migration_position_backfill'",
                    [],
                    |row| row.get::<_, i32>(0),
                )
                .unwrap_or(0)
                > 0;
            if !already_done {
                if let Err(e) = conn.execute_batch(
                    "UPDATE downloads SET position = (
                        SELECT COUNT(*) FROM downloads d2 WHERE d2.created_at < downloads.created_at
                     );
                     INSERT OR IGNORE INTO settings (key, value) VALUES ('migration_position_backfill', '1');",
                ) {
                    tracing::warn!("migration position backfill failed: {}", e);
                }
            }
        }

        // One-time migration: create initial download_folders entry from download_dir
        {
            let already_done: bool = conn
                .query_row(
                    "SELECT COUNT(*) FROM settings WHERE key = 'migration_download_folders'",
                    [],
                    |row| row.get::<_, i32>(0),
                )
                .unwrap_or(0)
                > 0;
            if !already_done {
                let has_folders: bool = conn
                    .query_row(
                        "SELECT COUNT(*) FROM settings WHERE key = 'download_folders'",
                        [],
                        |row| row.get::<_, i32>(0),
                    )
                    .unwrap_or(0)
                    > 0;
                if !has_folders {
                    let dir: String = conn
                        .query_row(
                            "SELECT value FROM settings WHERE key = 'download_dir'",
                            [],
                            |row| row.get(0),
                        )
                        .unwrap_or_else(|_| "/downloads".to_string());
                    let folder = serde_json::json!([{
                        "id": uuid::Uuid::new_v4().to_string(),
                        "label": "Default",
                        "path": dir,
                        "is_default": true,
                    }]);
                    if let Err(e) = conn.execute(
                        "INSERT OR IGNORE INTO settings (key, value) VALUES (?1, ?2)",
                        rusqlite::params![&"download_folders", &folder.to_string()],
                    ) {
                        tracing::warn!("migration download_folders failed: {}", e);
                    }
                }
                if let Err(e) = conn.execute(
                    "INSERT OR IGNORE INTO settings (key, value) VALUES (?1, ?2)",
                    rusqlite::params![&"migration_download_folders", &"1"],
                ) {
                    tracing::warn!("migration download_folders marker failed: {}", e);
                }
            }
        }

        // One-time migration: add tags column to downloads table.
        // Guarded by a settings flag so it only runs once.
        {
            let already_done: bool = conn
                .query_row(
                    "SELECT COUNT(*) FROM settings WHERE key = 'migration_tags_column'",
                    [],
                    |row| row.get::<_, i32>(0),
                )
                .unwrap_or(0)
                > 0;
            if !already_done {
                if let Err(e) = conn.execute_batch(
                    "ALTER TABLE downloads ADD COLUMN tags TEXT DEFAULT '';
                     INSERT OR IGNORE INTO settings (key, value) VALUES ('migration_tags_column', '1');",
                ) {
                    tracing::warn!("tags column migration failed: {}", e);
                }
            }
        }

        conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_downloads_info_hash ON downloads(info_hash);
             CREATE INDEX IF NOT EXISTS idx_history_created_at ON download_history(created_at);
             CREATE INDEX IF NOT EXISTS idx_api_keys_hash ON api_keys(key_hash);
             CREATE INDEX IF NOT EXISTS idx_api_keys_user ON api_keys(user_id);",
        )?;

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }
}
