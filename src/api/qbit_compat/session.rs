use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;

pub struct Session {
    pub username: String,
    pub role: String,
    pub created_at: Instant,
}

pub struct SessionStore {
    sessions: Arc<RwLock<HashMap<String, Session>>>,
}

const SESSION_TTL_SECS: u64 = 86400; // 24 hours
const SWEEP_INTERVAL_SECS: u64 = 300; // 5 minutes

impl Default for SessionStore {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionStore {
    pub fn new() -> Self {
        let sessions = Arc::new(RwLock::new(HashMap::new()));
        let store = Self {
            sessions: sessions.clone(),
        };
        // Background sweeper removes expired entries so hot-path ops stay short.
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(
                SWEEP_INTERVAL_SECS,
            ));
            // Skip the immediate first tick.
            ticker.tick().await;
            loop {
                ticker.tick().await;
                let mut guard = sessions.write().await;
                guard.retain(|_, s| s.created_at.elapsed().as_secs() <= SESSION_TTL_SECS);
            }
        });
        store
    }

    pub async fn create(&self, username: String, role: String) -> String {
        let sid = uuid::Uuid::new_v4().to_string().replace("-", "");
        let session = Session {
            username,
            role,
            created_at: Instant::now(),
        };
        let mut sessions = self.sessions.write().await;
        sessions.insert(sid.clone(), session);
        sid
    }

    pub async fn validate(&self, sid: &str) -> Option<(String, String)> {
        let sessions = self.sessions.read().await;
        let session = sessions.get(sid)?;
        if session.created_at.elapsed().as_secs() <= SESSION_TTL_SECS {
            Some((session.username.clone(), session.role.clone()))
        } else {
            // Expired entries are swept by the background task; drop-and-upgrade
            // would add contention, so we just report absent here.
            None
        }
    }

    pub async fn remove(&self, sid: &str) {
        self.sessions.write().await.remove(sid);
    }

    /// Invalidate every session belonging to `username`. Called when a user is
    /// deleted or changes their password so stale sessions cannot keep access.
    pub async fn remove_by_username(&self, username: &str) {
        self.sessions
            .write()
            .await
            .retain(|_, s| s.username != username);
    }
}
