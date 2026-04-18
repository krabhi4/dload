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
            );",
        )?;

        let _ = conn.execute_batch(
            "ALTER TABLE downloads ADD COLUMN info_hash TEXT;
             ALTER TABLE downloads ADD COLUMN category TEXT;
             ALTER TABLE downloads ADD COLUMN content_path TEXT;",
        );
        let _ = conn.execute_batch(
            "ALTER TABLE downloads ADD COLUMN http_mirror_status TEXT;
             ALTER TABLE downloads ADD COLUMN http_mirror_url TEXT;",
        );
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
                let _ = conn.execute_batch(
                    "UPDATE downloads SET restart_resume = 1
                     WHERE protocol = 'Torrent'
                     AND status IN ('Downloading', 'Seeding', 'Paused', 'Queued')
                     AND restart_resume = 0;
                     INSERT OR IGNORE INTO settings (key, value) VALUES ('migration_restart_resume', '1');",
                );
            }
        }
        let _ = conn.execute_batch("ALTER TABLE downloads ADD COLUMN position INTEGER DEFAULT 0;");
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
                let _ = conn.execute_batch(
                    "UPDATE downloads SET position = (
                        SELECT COUNT(*) FROM downloads d2 WHERE d2.created_at < downloads.created_at
                     );
                     INSERT OR IGNORE INTO settings (key, value) VALUES ('migration_position_backfill', '1');",
                );
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
                    let _ = conn.execute(
                        "INSERT OR IGNORE INTO settings (key, value) VALUES (?1, ?2)",
                        rusqlite::params![&"download_folders", &folder.to_string()],
                    );
                }
                let _ = conn.execute(
                    "INSERT OR IGNORE INTO settings (key, value) VALUES (?1, ?2)",
                    rusqlite::params![&"migration_download_folders", &"1"],
                );
            }
        }

        conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_downloads_info_hash ON downloads(info_hash);
             CREATE INDEX IF NOT EXISTS idx_history_created_at ON download_history(created_at);",
        )?;

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }
}
