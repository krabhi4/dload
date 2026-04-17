use crate::domain::{Download, DownloadStatus, Protocol, Role, Settings, User};
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
             speed, progress, status, protocol, connections, created_at, completed_at, error_message,
             info_hash, category, content_path, http_mirror_status, http_mirror_url, restart_resume, position)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21)",
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
                download.info_hash,
                download.category,
                download.content_path,
                download.http_mirror_status,
                download.http_mirror_url,
                download.restart_resume as i32,
                download.position,
            ],
        )?;
        Ok(())
    }

    pub fn update_download(&self, download: &Download) -> anyhow::Result<()> {
        let conn = self.db.conn.lock().unwrap();
        conn.execute(
            "UPDATE downloads SET filename=?1, save_path=?2, total_size=?3, downloaded_size=?4,
             speed=?5, progress=?6, status=?7, completed_at=?8, error_message=?9, connections=?10,
             info_hash=?11, category=?12, content_path=?13, http_mirror_status=?14, http_mirror_url=?15,
             restart_resume=?16, position=?17
             WHERE id=?18",
            params![
                download.filename,
                download.save_path,
                download.total_size,
                download.downloaded_size,
                download.speed,
                download.progress,
                format!("{:?}", download.status),
                download.completed_at.map(|d| d.to_rfc3339()),
                download.error_message,
                download.connections,
                download.info_hash,
                download.category,
                download.content_path,
                download.http_mirror_status,
                download.http_mirror_url,
                download.restart_resume as i32,
                download.position,
                download.id,
            ],
        )?;
        Ok(())
    }

    pub fn get_all_downloads(&self) -> anyhow::Result<Vec<Download>> {
        let conn = self.db.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, url, filename, save_path, total_size, downloaded_size, speed,
             progress, status, protocol, connections, created_at, completed_at, error_message,
             info_hash, category, content_path, http_mirror_status, http_mirror_url, restart_resume,
             position
             FROM downloads ORDER BY position ASC, created_at ASC",
        )?;

        let downloads = stmt
            .query_map([], |row| {
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
                    status: serde_json::from_str(&format!("\"{}\"", status_str))
                        .unwrap_or(DownloadStatus::Queued),
                    protocol: serde_json::from_str(&format!("\"{}\"", protocol_str))
                        .unwrap_or(Protocol::Http),
                    upload_speed: 0,
                    connections: row.get(10)?,
                    peers: 0,
                    seeds: 0,
                    eta: None,
                    created_at: chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>(11)?)
                        .map(|d| d.with_timezone(&chrono::Utc))
                        .unwrap_or_else(|_| chrono::Utc::now()),
                    completed_at: row
                        .get::<_, Option<String>>(12)?
                        .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
                        .map(|d| d.with_timezone(&chrono::Utc)),
                    error_message: row.get(13)?,
                    info_hash: row.get(14)?,
                    category: row.get(15)?,
                    content_path: row.get(16)?,
                    http_mirror_status: row.get(17)?,
                    http_mirror_url: row.get(18)?,
                    restart_resume: row.get::<_, i32>(19).unwrap_or(0) != 0,
                    position: row.get::<_, i32>(20).unwrap_or(0),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(downloads)
    }

    pub fn update_positions(&self, positions: &[(String, i32)]) -> anyhow::Result<()> {
        let conn = self.db.conn.lock().unwrap();
        let tx = conn.unchecked_transaction()?;
        for (id, pos) in positions {
            tx.execute(
                "UPDATE downloads SET position = ?1 WHERE id = ?2",
                params![pos, id],
            )?;
        }
        tx.commit()?;
        Ok(())
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
                "max_connections_per_file" => {
                    settings.max_connections_per_file = value.parse().unwrap_or(8)
                }
                "chunk_size" | "min_split_size" => {
                    settings.min_split_size = value.parse().unwrap_or(20 * 1024 * 1024)
                }
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
            (
                "max_connections_per_file",
                settings.max_connections_per_file.to_string(),
            ),
            ("min_split_size", settings.min_split_size.to_string()),
            ("username", settings.username.clone()),
            ("port", settings.port.to_string()),
        ];

        let tx = conn.unchecked_transaction()?;
        for (key, value) in pairs {
            tx.execute(
                "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
                params![key, value],
            )?;
        }
        // Clean up legacy key from older versions
        tx.execute("DELETE FROM settings WHERE key = 'chunk_size'", params![])?;
        tx.commit()?;
        Ok(())
    }

    pub fn insert_user(&self, user: &User) -> anyhow::Result<()> {
        let conn = self.db.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO users (id, username, password_hash, role, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                user.id,
                user.username,
                user.password_hash,
                user.role.as_str(),
                user.created_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    /// Atomically insert the first user only if no users exist yet.
    /// Returns Ok(true) if inserted, Ok(false) if users already exist.
    pub fn insert_first_user(&self, user: &User) -> anyhow::Result<bool> {
        let conn = self.db.conn.lock().unwrap();
        let rows = conn.execute(
            "INSERT INTO users (id, username, password_hash, role, created_at)
             SELECT ?1, ?2, ?3, ?4, ?5
             WHERE NOT EXISTS (SELECT 1 FROM users)",
            params![
                user.id,
                user.username,
                user.password_hash,
                user.role.as_str(),
                user.created_at.to_rfc3339(),
            ],
        )?;
        Ok(rows > 0)
    }

    pub fn get_user_by_username(&self, username: &str) -> anyhow::Result<Option<User>> {
        let conn = self.db.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, username, password_hash, role, created_at FROM users WHERE username = ?1",
        )?;
        let mut rows = stmt.query(params![username])?;

        if let Some(row) = rows.next()? {
            Ok(Some(User {
                id: row.get(0)?,
                username: row.get(1)?,
                password_hash: row.get(2)?,
                role: Role::parse(&row.get::<_, String>(3)?),
                created_at: chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>(4)?)
                    .map(|d| d.with_timezone(&chrono::Utc))
                    .unwrap_or_else(|_| chrono::Utc::now()),
            }))
        } else {
            Ok(None)
        }
    }

    pub fn get_user_by_id(&self, id: &str) -> anyhow::Result<Option<User>> {
        let conn = self.db.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, username, password_hash, role, created_at FROM users WHERE id = ?1",
        )?;
        let mut rows = stmt.query(params![id])?;

        if let Some(row) = rows.next()? {
            Ok(Some(User {
                id: row.get(0)?,
                username: row.get(1)?,
                password_hash: row.get(2)?,
                role: Role::parse(&row.get::<_, String>(3)?),
                created_at: chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>(4)?)
                    .map(|d| d.with_timezone(&chrono::Utc))
                    .unwrap_or_else(|_| chrono::Utc::now()),
            }))
        } else {
            Ok(None)
        }
    }

    pub fn get_all_users(&self) -> anyhow::Result<Vec<User>> {
        let conn = self.db.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT id, username, password_hash, role, created_at FROM users ORDER BY created_at ASC")?;
        let users = stmt
            .query_map([], |row| {
                Ok(User {
                    id: row.get(0)?,
                    username: row.get(1)?,
                    password_hash: row.get(2)?,
                    role: Role::parse(&row.get::<_, String>(3)?),
                    created_at: chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>(4)?)
                        .map(|d| d.with_timezone(&chrono::Utc))
                        .unwrap_or_else(|_| chrono::Utc::now()),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(users)
    }

    pub fn update_user_password(&self, username: &str, password_hash: &str) -> anyhow::Result<()> {
        let conn = self.db.conn.lock().unwrap();
        let rows = conn.execute(
            "UPDATE users SET password_hash = ?1 WHERE username = ?2",
            params![password_hash, username],
        )?;
        if rows == 0 {
            anyhow::bail!("User not found");
        }
        Ok(())
    }

    pub fn delete_user(&self, id: &str) -> anyhow::Result<()> {
        let conn = self.db.conn.lock().unwrap();
        conn.execute("DELETE FROM users WHERE id = ?1", params![id])?;
        Ok(())
    }

    // ─── History ────────────────────────────────────────

    pub fn insert_history(&self, download: &Download) -> anyhow::Result<()> {
        let conn = self.db.conn.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO download_history (id, url, filename, total_size, status, protocol, created_at, completed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                download.id,
                download.url,
                download.filename,
                download.total_size,
                format!("{:?}", download.status),
                format!("{:?}", download.protocol),
                download.created_at.to_rfc3339(),
                download.completed_at.map(|d| d.to_rfc3339()),
            ],
        )?;
        Ok(())
    }

    pub fn update_history(&self, download: &Download) -> anyhow::Result<()> {
        let conn = self.db.conn.lock().unwrap();
        conn.execute(
            "UPDATE download_history SET filename = ?2, total_size = ?3, status = ?4, completed_at = ?5
             WHERE id = ?1",
            params![
                download.id,
                download.filename,
                download.total_size,
                format!("{:?}", download.status),
                download.completed_at.map(|d| d.to_rfc3339()),
            ],
        )?;
        Ok(())
    }

    pub fn get_history_page(
        &self,
        limit: i64,
        offset: i64,
    ) -> anyhow::Result<Vec<serde_json::Value>> {
        let conn = self.db.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, url, filename, total_size, status, protocol, created_at, completed_at
             FROM download_history ORDER BY created_at DESC LIMIT ?1 OFFSET ?2",
        )?;

        let rows = stmt
            .query_map(params![limit, offset], |row| {
                Ok(serde_json::json!({
                    "id": row.get::<_, String>(0)?,
                    "url": row.get::<_, String>(1)?,
                    "filename": row.get::<_, String>(2)?,
                    "total_size": row.get::<_, u64>(3)?,
                    "status": row.get::<_, String>(4)?,
                    "protocol": row.get::<_, String>(5)?,
                    "created_at": row.get::<_, String>(6)?,
                    "completed_at": row.get::<_, Option<String>>(7)?,
                }))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(rows)
    }

    pub fn delete_history(&self, id: &str) -> anyhow::Result<()> {
        let conn = self.db.conn.lock().unwrap();
        conn.execute("DELETE FROM download_history WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn delete_all_history(&self) -> anyhow::Result<()> {
        let conn = self.db.conn.lock().unwrap();
        conn.execute("DELETE FROM download_history", [])?;
        Ok(())
    }
}
