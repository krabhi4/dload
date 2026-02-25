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
             info_hash, category, content_path)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
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
            ],
        )?;
        Ok(())
    }

    pub fn update_download(&self, download: &Download) -> anyhow::Result<()> {
        let conn = self.db.conn.lock().unwrap();
        conn.execute(
            "UPDATE downloads SET filename=?1, save_path=?2, total_size=?3, downloaded_size=?4,
             speed=?5, progress=?6, status=?7, completed_at=?8, error_message=?9, connections=?10,
             info_hash=?11, category=?12, content_path=?13 WHERE id=?14",
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
             info_hash, category, content_path
             FROM downloads ORDER BY created_at DESC",
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
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

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
                "max_connections_per_file" => {
                    settings.max_connections_per_file = value.parse().unwrap_or(4)
                }
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
            (
                "max_connections_per_file",
                settings.max_connections_per_file.to_string(),
            ),
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
                role: Role::from_str(&row.get::<_, String>(3)?),
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
                role: Role::from_str(&row.get::<_, String>(3)?),
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
                    role: Role::from_str(&row.get::<_, String>(3)?),
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
}
