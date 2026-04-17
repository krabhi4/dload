use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
// tokio::time::Instant is controllable by `#[tokio::test(start_paused = true)]`
// and behaves identically to std::time::Instant in production.
use tokio::time::Instant;

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
            let mut ticker =
                tokio::time::interval(std::time::Duration::from_secs(SWEEP_INTERVAL_SECS));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn create_returns_unique_sids() {
        let store = SessionStore::new();
        let a = store.create("alice".into(), "ADMIN".into()).await;
        let b = store.create("bob".into(), "ADMIN".into()).await;
        assert_ne!(a, b);
        // SID is a hex-ish string from uuid v4 without dashes
        assert_eq!(a.len(), 32);
    }

    #[tokio::test]
    async fn validate_returns_username_and_role_for_fresh_session() {
        let store = SessionStore::new();
        let sid = store.create("alice".into(), "ADMIN".into()).await;
        let got = store.validate(&sid).await;
        assert_eq!(got, Some(("alice".into(), "ADMIN".into())));
    }

    #[tokio::test]
    async fn validate_returns_none_for_unknown_sid() {
        let store = SessionStore::new();
        assert_eq!(store.validate("does-not-exist").await, None);
    }

    #[tokio::test]
    async fn remove_clears_session() {
        let store = SessionStore::new();
        let sid = store.create("alice".into(), "ADMIN".into()).await;
        assert!(store.validate(&sid).await.is_some());
        store.remove(&sid).await;
        assert!(store.validate(&sid).await.is_none());
    }

    #[tokio::test]
    async fn remove_by_username_clears_all_sessions_for_that_user() {
        let store = SessionStore::new();
        let a = store.create("alice".into(), "ADMIN".into()).await;
        let b = store.create("alice".into(), "ADMIN".into()).await;
        let c = store.create("bob".into(), "ADMIN".into()).await;
        store.remove_by_username("alice").await;
        assert!(store.validate(&a).await.is_none());
        assert!(store.validate(&b).await.is_none());
        assert!(
            store.validate(&c).await.is_some(),
            "bob's session untouched"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn validate_rejects_expired_session_before_sweeper_runs() {
        let store = SessionStore::new();
        let sid = store.create("alice".into(), "ADMIN".into()).await;
        assert!(store.validate(&sid).await.is_some());
        // Jump past the 24h TTL without letting the sweeper run
        tokio::time::advance(std::time::Duration::from_secs(SESSION_TTL_SECS + 1)).await;
        assert!(
            store.validate(&sid).await.is_none(),
            "validate() must not return an expired session even if the sweeper hasn't run"
        );
    }
}
