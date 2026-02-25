use std::collections::HashMap;
use std::time::Instant;
use tokio::sync::RwLock;

pub struct Session {
    pub username: String,
    pub role: String,
    pub created_at: Instant,
}

pub struct SessionStore {
    sessions: RwLock<HashMap<String, Session>>,
}

const SESSION_TTL_SECS: u64 = 86400; // 24 hours

impl SessionStore {
    pub fn new() -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
        }
    }

    pub async fn create(&self, username: String, role: String) -> String {
        let sid = uuid::Uuid::new_v4().to_string().replace("-", "");
        let session = Session {
            username,
            role,
            created_at: Instant::now(),
        };
        let mut sessions = self.sessions.write().await;
        // Sweep expired sessions to prevent memory leak
        sessions.retain(|_, s| s.created_at.elapsed().as_secs() <= SESSION_TTL_SECS);
        sessions.insert(sid.clone(), session);
        sid
    }

    pub async fn validate(&self, sid: &str) -> Option<(String, String)> {
        // Check with read lock first
        {
            let sessions = self.sessions.read().await;
            let session = sessions.get(sid)?;
            if session.created_at.elapsed().as_secs() <= SESSION_TTL_SECS {
                return Some((session.username.clone(), session.role.clone()));
            }
        }
        // Expired — remove with write lock
        self.sessions.write().await.remove(sid);
        None
    }

    pub async fn remove(&self, sid: &str) {
        self.sessions.write().await.remove(sid);
    }
}
