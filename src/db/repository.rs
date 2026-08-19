use crate::domain::{
    ApiKey, Download, DownloadFolder, DownloadStatus, Protocol, Role, Settings, User,
};
use rusqlite::params;
use rusqlite::OptionalExtension;
use std::collections::HashMap;
use std::sync::Arc;

pub struct Repository {
    db: Arc<crate::db::Database>,
}

/// Fields for inserting a new API key (the SHA-256 hash, not the plaintext).
pub struct NewApiKey {
    pub id: String,
    pub user_id: String,
    pub name: String,
    pub key_hash: String,
    pub prefix: String,
    pub created_at: String,
}

#[derive(Debug)]
pub enum InsertUserError {
    UsernameConflict,
    Other(anyhow::Error),
}

impl std::fmt::Display for InsertUserError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InsertUserError::UsernameConflict => write!(f, "Username already exists"),
            InsertUserError::Other(e) => write!(f, "{}", e),
        }
    }
}

impl std::error::Error for InsertUserError {}

impl Repository {
    pub fn new(db: Arc<crate::db::Database>) -> Self {
        Self { db }
    }

    fn is_unique_constraint_error(e: &rusqlite::Error) -> bool {
        if let rusqlite::Error::SqliteFailure(err, _) = e {
            return err.extended_code == 2067;
        }
        false
    }

    pub fn insert_download(&self, download: &Download) -> anyhow::Result<()> {
        let conn = self.db.conn.lock().unwrap();
        conn.execute(
             "INSERT INTO downloads (id, url, filename, save_path, total_size, downloaded_size,
              speed, progress, status, protocol, connections, created_at, completed_at, error_message,
              info_hash, category, content_path, http_mirror_status, http_mirror_url, restart_resume, position,
              download_folder, tags)
              VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23)",
            params![
                download.id,
                download.url,
                download.filename,
                download.save_path,
                download.total_size as i64,
                download.downloaded_size as i64,
                download.speed as i64,
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
                download.download_folder,
                download.tags_to_string(),
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
              restart_resume=?16, position=?17, download_folder=?18, tags=?19
              WHERE id=?20",
            params![
                download.filename,
                download.save_path,
                download.total_size as i64,
                download.downloaded_size as i64,
                download.speed as i64,
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
                download.download_folder,
                download.tags_to_string(),
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
              position, download_folder, tags
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
                    total_size: row.get::<_, i64>(4)? as u64,
                    downloaded_size: row.get::<_, i64>(5)? as u64,
                    speed: row.get::<_, i64>(6)? as u64,
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
                    tags: row
                        .get::<_, Option<String>>(22)?
                        .map(|s| Download::tags_from_string(&s))
                        .unwrap_or_default(),
                    content_path: row.get(16)?,
                    http_mirror_status: row.get(17)?,
                    http_mirror_url: row.get(18)?,
                    restart_resume: row.get::<_, i32>(19).unwrap_or(0) != 0,
                    position: row.get::<_, i32>(20).unwrap_or(0),
                    download_folder: row.get::<_, Option<String>>(21)?.unwrap_or_default(),
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
                "download_folders" => {
                    if let Ok(folders) = serde_json::from_str::<Vec<DownloadFolder>>(&value) {
                        settings.download_folders = folders;
                    }
                }
                _ => {}
            }
        }
        Ok(settings)
    }

    pub fn save_settings(&self, settings: &Settings) -> anyhow::Result<()> {
        let conn = self.db.conn.lock().unwrap();
        let folders_json =
            serde_json::to_string(&settings.download_folders).unwrap_or_else(|_| "[]".to_string());
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
            ("download_folders", folders_json),
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

    pub fn get_qbit_categories(&self) -> anyhow::Result<HashMap<String, Option<String>>> {
        let conn = self.db.conn.lock().unwrap();
        let value: rusqlite::Result<String> = conn.query_row(
            "SELECT value FROM settings WHERE key = 'qbit_categories'",
            [],
            |row| row.get(0),
        );
        match value {
            Ok(s) => Ok(serde_json::from_str(&s).unwrap_or_default()),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(HashMap::new()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn save_qbit_categories(
        &self,
        cats: &HashMap<String, Option<String>>,
    ) -> anyhow::Result<()> {
        let conn = self.db.conn.lock().unwrap();
        let json = serde_json::to_string(cats)?;
        conn.execute(
            "INSERT OR REPLACE INTO settings (key, value) VALUES ('qbit_categories', ?1)",
            params![json],
        )?;
        Ok(())
    }

    pub fn insert_user(&self, user: &User) -> Result<(), InsertUserError> {
        let conn = self.db.conn.lock().unwrap();
        match conn.execute(
            "INSERT INTO users (id, username, password_hash, role, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                user.id,
                user.username,
                user.password_hash,
                user.role.as_str(),
                user.created_at.to_rfc3339(),
            ],
        ) {
            Ok(_) => Ok(()),
            Err(e) => {
                // SQLite UNIQUE constraint violation
                if Self::is_unique_constraint_error(&e) {
                    Err(InsertUserError::UsernameConflict)
                } else {
                    Err(InsertUserError::Other(e.into()))
                }
            }
        }
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
            "SELECT id, username, password_hash, role, created_at, token_version
             FROM users WHERE username = ?1",
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
                token_version: row.get(5)?,
            }))
        } else {
            Ok(None)
        }
    }


    pub fn get_all_users(&self) -> anyhow::Result<Vec<User>> {
        let conn = self.db.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, username, password_hash, role, created_at, token_version
             FROM users ORDER BY created_at ASC",
        )?;
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
                    token_version: row.get(5)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(users)
    }

    pub fn update_user_password_and_bump_version(
        &self,
        username: &str,
        password_hash: &str,
    ) -> anyhow::Result<i64> {
        let conn = self.db.conn.lock().unwrap();
        let rows = conn.execute(
            "UPDATE users SET password_hash = ?1, token_version = token_version + 1 WHERE username = ?2",
            params![password_hash, username],
        )?;
        if rows == 0 {
            anyhow::bail!("User not found");
        }
        let new_version: i64 = conn.query_row(
            "SELECT token_version FROM users WHERE username = ?1",
            params![username],
            |row| row.get(0),
        )?;
        Ok(new_version)
    }

    pub fn delete_user_guard_last_admin(
        &self,
        id: &str,
    ) -> anyhow::Result<Option<Option<String>>> {
        let conn = self.db.conn.lock().unwrap();

        let target: Option<(String, String, Role)> = conn
            .query_row(
                "SELECT id, username, role FROM users WHERE id = ?1",
                params![id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        Role::parse(&row.get::<_, String>(2)?),
                    ))
                },
            )
            .optional()?;

        let target = match target {
            Some(t) => t,
            None => return Ok(Some(None)),
        };

        if target.2 == Role::Admin {
            let other_admins: i64 = conn.query_row(
                "SELECT COUNT(*) FROM users WHERE role = 'ADMIN' AND id != ?1",
                params![id],
                |row| row.get(0),
            )?;
            if other_admins == 0 {
                return Ok(None);
            }
        }

        conn.execute("DELETE FROM users WHERE id = ?1", params![id])?;
        conn.execute("DELETE FROM api_keys WHERE user_id = ?1", params![id])?;

        Ok(Some(Some(target.1)))
    }

    // ─── API keys ───────────────────────────────────────

    pub fn get_user_by_api_key_hash(&self, key_hash: &str) -> anyhow::Result<Option<User>> {
        let conn = self.db.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT u.id, u.username, u.password_hash, u.role, u.created_at, u.token_version
             FROM users u JOIN api_keys k ON k.user_id = u.id
             WHERE k.key_hash = ?1",
        )?;
        let mut rows = stmt.query(params![key_hash])?;
        if let Some(row) = rows.next()? {
            Ok(Some(User {
                id: row.get(0)?,
                username: row.get(1)?,
                password_hash: row.get(2)?,
                role: Role::parse(&row.get::<_, String>(3)?),
                created_at: chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>(4)?)
                    .map(|d| d.with_timezone(&chrono::Utc))
                    .unwrap_or_else(|_| chrono::Utc::now()),
                token_version: row.get(5)?,
            }))
        } else {
            Ok(None)
        }
    }

    // Stamp last_used_at only if unset/older than cutoff, so the hot auth path
    // doesn't write on every request.
    pub fn touch_api_key(&self, key_hash: &str, now: &str, cutoff: &str) -> anyhow::Result<()> {
        let conn = self.db.conn.lock().unwrap();
        conn.execute(
            "UPDATE api_keys SET last_used_at = ?1
             WHERE key_hash = ?2 AND (last_used_at IS NULL OR last_used_at < ?3)",
            params![now, key_hash, cutoff],
        )?;
        Ok(())
    }

    pub fn list_api_keys_for_user(&self, user_id: &str) -> anyhow::Result<Vec<ApiKey>> {
        let conn = self.db.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, user_id, name, prefix, created_at, last_used_at
             FROM api_keys WHERE user_id = ?1 ORDER BY created_at ASC",
        )?;
        let keys = stmt
            .query_map(params![user_id], |row| {
                Ok(ApiKey {
                    id: row.get(0)?,
                    user_id: row.get(1)?,
                    name: row.get(2)?,
                    prefix: row.get(3)?,
                    created_at: chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>(4)?)
                        .map(|d| d.with_timezone(&chrono::Utc))
                        .unwrap_or_else(|_| chrono::Utc::now()),
                    last_used_at: row
                        .get::<_, Option<String>>(5)?
                        .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
                        .map(|d| d.with_timezone(&chrono::Utc)),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(keys)
    }

    // Count + insert in one statement under the lock, so concurrent creates
    // can't race past the cap. Returns false if the cap was reached.
    pub fn insert_api_key_if_under_cap(&self, k: &NewApiKey, max: i64) -> anyhow::Result<bool> {
        let conn = self.db.conn.lock().unwrap();
        let rows = conn.execute(
            "INSERT INTO api_keys (id, user_id, name, key_hash, prefix, created_at)
             SELECT ?1, ?2, ?3, ?4, ?5, ?6
             WHERE (SELECT COUNT(*) FROM api_keys WHERE user_id = ?2) < ?7",
            params![
                k.id,
                k.user_id,
                k.name,
                k.key_hash,
                k.prefix,
                k.created_at,
                max
            ],
        )?;
        Ok(rows > 0)
    }

    // Scoped to its owner so a user can't revoke another's key.
    pub fn delete_api_key(&self, id: &str, user_id: &str) -> anyhow::Result<bool> {
        let conn = self.db.conn.lock().unwrap();
        let rows = conn.execute(
            "DELETE FROM api_keys WHERE id = ?1 AND user_id = ?2",
            params![id, user_id],
        )?;
        Ok(rows > 0)
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
                download.total_size as i64,
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
                download.total_size as i64,
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
                    "total_size": row.get::<_, i64>(3)? as u64,
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
