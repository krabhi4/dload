pub mod repository;

use rusqlite::Connection;
use std::sync::Mutex;

pub struct Database {
    pub(crate) conn: Mutex<Connection>,
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
             ALTER TABLE downloads ADD COLUMN content_path TEXT;"
        );
        conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_downloads_info_hash ON downloads(info_hash);"
        )?;

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }
}
