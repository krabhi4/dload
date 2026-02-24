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
            );"
        )?;
        
        Ok(Self { conn: Mutex::new(conn) })
    }
}
