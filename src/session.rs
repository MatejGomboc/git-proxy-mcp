//! Session management for tracking cloned repositories.
//!
//! This module provides session tracking for the credential relay.
//! Sessions track metadata about cloned repos WITHOUT storing any files.
//!
//! # Security
//!
//! - No file paths stored (we don't write source files to disk)
//! - No credentials stored (handled by git2 callbacks)
//! - Only metadata: URL, branch, commit hash

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use tracing::{debug, info};

use crate::git2_ops::auth::sanitize_url_for_logging;

/// A session tracking a cloned repository.
#[derive(Debug, Clone)]
pub struct RepoSession {
    /// Repository URL (sanitised for display, original for operations).
    url: String,

    /// Current branch name.
    pub branch: String,

    /// Last known commit hash.
    pub last_commit: String,

    /// When this session was created.
    created_at: Instant,

    /// When this session was last accessed.
    last_accessed: Instant,
}

impl RepoSession {
    /// Create a new session.
    #[must_use]
    pub fn new(url: String, branch: String, commit: String) -> Self {
        let now = Instant::now();
        Self {
            url,
            branch,
            last_commit: commit,
            created_at: now,
            last_accessed: now,
        }
    }

    /// Get the repository URL (original, for operations).
    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Get the repository URL sanitised for logging/display.
    #[must_use]
    pub fn sanitized_url(&self) -> String {
        sanitize_url_for_logging(&self.url)
    }

    /// Check if this session has expired given the configured maximum age.
    #[must_use]
    pub fn is_expired(&self, max_age: Duration) -> bool {
        self.last_accessed.elapsed() > max_age
    }

    /// Get the session age.
    #[must_use]
    pub fn age(&self) -> Duration {
        self.created_at.elapsed()
    }

    /// Touch the session to update last accessed time.
    pub fn touch(&mut self) {
        self.last_accessed = Instant::now();
    }

    /// Update the last commit.
    pub fn update_commit(&mut self, commit: String) {
        self.last_commit = commit;
        self.touch();
    }
}

/// Thread-safe session manager.
#[derive(Debug, Clone)]
pub struct SessionManager {
    sessions: Arc<RwLock<HashMap<String, RepoSession>>>,
    /// Maximum session age before automatic cleanup.
    session_max_age: Duration,
    /// Maximum number of sessions to prevent memory exhaustion.
    max_sessions: usize,
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new(Duration::from_secs(3600), 100)
    }
}

impl SessionManager {
    /// Create a new session manager with the given maximum age and session limit.
    #[must_use]
    pub fn new(session_max_age: Duration, max_sessions: usize) -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            session_max_age,
            max_sessions,
        }
    }

    /// Generate a session ID from URL and branch.
    #[must_use]
    pub fn session_id(url: &str, branch: &str) -> String {
        // Use sanitised URL for the key to avoid storing credentials
        let sanitized = sanitize_url_for_logging(url);
        format!("{sanitized}@{branch}")
    }

    /// Create or update a session after a clone operation.
    ///
    /// Updating an existing session (same URL and branch) overwrites it in
    /// place and is never rejected for capacity, because it does not grow the
    /// session map. Only a genuinely new session is subject to the
    /// `max_sessions` limit.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError::TooManySessions`] if a *new* session would
    /// exceed `max_sessions` and no expired sessions could be evicted to make
    /// room, or [`SessionError::LockPoisoned`] if the session lock is poisoned.
    #[allow(clippy::significant_drop_tightening)]
    pub fn create_session(
        &self,
        url: &str,
        branch: &str,
        commit: &str,
    ) -> Result<String, SessionError> {
        let session_id = Self::session_id(url, branch);

        let mut sessions = self
            .sessions
            .write()
            .map_err(|_| SessionError::LockPoisoned)?;

        // Enforce the capacity limit only when inserting a *new* session.
        // Re-creating a session that already exists (same URL + branch)
        // overwrites it in place and does not grow the map, so it must not be
        // rejected just because we are at capacity.
        if !sessions.contains_key(&session_id) && sessions.len() >= self.max_sessions {
            // Try to make room by evicting expired sessions first.
            self.cleanup_expired_internal(&mut sessions);

            // Still at capacity after cleanup?
            if sessions.len() >= self.max_sessions {
                return Err(SessionError::TooManySessions {
                    max: self.max_sessions,
                });
            }
        }

        let session = RepoSession::new(url.to_string(), branch.to_string(), commit.to_string());

        info!(
            session_id = %session_id,
            url = %session.sanitized_url(),
            branch = %branch,
            commit = %commit,
            "session created"
        );

        sessions.insert(session_id.clone(), session);

        Ok(session_id)
    }

    /// Get a session by ID, updating its last accessed time.
    ///
    /// # Errors
    ///
    /// Returns an error if the lock is poisoned.
    #[allow(clippy::significant_drop_tightening)]
    pub fn get_session(&self, session_id: &str) -> Result<Option<RepoSession>, SessionError> {
        let mut sessions = self
            .sessions
            .write()
            .map_err(|_| SessionError::LockPoisoned)?;

        if let Some(session) = sessions.get_mut(session_id) {
            if session.is_expired(self.session_max_age) {
                debug!(session_id = %session_id, "session expired, removing");
                sessions.remove(session_id);
                return Ok(None);
            }

            session.touch();
            return Ok(Some(session.clone()));
        }

        Ok(None)
    }

    /// Update a session's commit after a push operation.
    ///
    /// # Errors
    ///
    /// Returns an error if the session doesn't exist or the lock is poisoned.
    #[allow(clippy::option_if_let_else)]
    pub fn update_session_commit(
        &self,
        session_id: &str,
        new_commit: &str,
    ) -> Result<(), SessionError> {
        let mut sessions = self
            .sessions
            .write()
            .map_err(|_| SessionError::LockPoisoned)?;

        if let Some(session) = sessions.get_mut(session_id) {
            session.update_commit(new_commit.to_string());
            debug!(session_id = %session_id, commit = %new_commit, "session commit updated");
            Ok(())
        } else {
            Err(SessionError::NotFound(session_id.to_string()))
        }
    }

    /// Remove a session.
    ///
    /// # Errors
    ///
    /// Returns an error if the lock is poisoned.
    #[allow(clippy::significant_drop_tightening)]
    pub fn remove_session(&self, session_id: &str) -> Result<bool, SessionError> {
        let mut sessions = self
            .sessions
            .write()
            .map_err(|_| SessionError::LockPoisoned)?;

        let removed = sessions.remove(session_id).is_some();

        if removed {
            debug!(session_id = %session_id, "session removed");
        }

        Ok(removed)
    }

    /// Get the number of active sessions.
    ///
    /// # Errors
    ///
    /// Returns an error if the lock is poisoned.
    pub fn session_count(&self) -> Result<usize, SessionError> {
        let sessions = self
            .sessions
            .read()
            .map_err(|_| SessionError::LockPoisoned)?;
        Ok(sessions.len())
    }

    /// Clean up expired sessions.
    ///
    /// # Errors
    ///
    /// Returns an error if the lock is poisoned.
    pub fn cleanup_expired(&self) -> Result<usize, SessionError> {
        let mut sessions = self
            .sessions
            .write()
            .map_err(|_| SessionError::LockPoisoned)?;

        Ok(self.cleanup_expired_internal(&mut sessions))
    }

    /// Internal cleanup helper (assumes lock is held).
    fn cleanup_expired_internal(&self, sessions: &mut HashMap<String, RepoSession>) -> usize {
        let max_age = self.session_max_age;
        let before = sessions.len();
        sessions.retain(|id, session| {
            let keep = !session.is_expired(max_age);
            if !keep {
                debug!(session_id = %id, "cleaning up expired session");
            }
            keep
        });
        let removed = before - sessions.len();
        if removed > 0 {
            info!(removed = removed, "cleaned up expired sessions");
        }
        removed
    }

    /// List all active session IDs (for debugging/status).
    ///
    /// # Errors
    ///
    /// Returns an error if the lock is poisoned.
    pub fn list_sessions(&self) -> Result<Vec<String>, SessionError> {
        let sessions = self
            .sessions
            .read()
            .map_err(|_| SessionError::LockPoisoned)?;
        Ok(sessions.keys().cloned().collect())
    }
}

/// Session management errors.
#[derive(Debug)]
pub enum SessionError {
    /// The session lock is poisoned (panic in another thread).
    LockPoisoned,

    /// Session not found.
    NotFound(String),

    /// Too many sessions active.
    TooManySessions { max: usize },
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LockPoisoned => write!(f, "session lock poisoned"),
            Self::NotFound(id) => write!(f, "session not found: {id}"),
            Self::TooManySessions { max } => {
                write!(f, "too many active sessions (max {max}), try again later")
            }
        }
    }
}

impl std::error::Error for SessionError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_and_get_session() {
        let manager = SessionManager::new(Duration::from_secs(3600), 100);

        let session_id = manager
            .create_session("https://github.com/owner/repo.git", "main", "abc123")
            .unwrap();

        assert!(!session_id.is_empty());

        let session = manager.get_session(&session_id).unwrap().unwrap();
        assert_eq!(session.branch, "main");
        assert_eq!(session.last_commit, "abc123");
    }

    #[test]
    fn session_id_sanitizes_credentials() {
        let id =
            SessionManager::session_id("https://user:secret@github.com/owner/repo.git", "main");

        // Should not contain the password
        assert!(!id.contains("secret"));
        assert!(id.contains("github.com"));
        assert!(id.contains("main"));
    }

    #[test]
    fn update_session_commit() {
        let manager = SessionManager::new(Duration::from_secs(3600), 100);

        let session_id = manager
            .create_session("https://github.com/owner/repo.git", "main", "abc123")
            .unwrap();

        manager
            .update_session_commit(&session_id, "def456")
            .unwrap();

        let session = manager.get_session(&session_id).unwrap().unwrap();
        assert_eq!(session.last_commit, "def456");
    }

    #[test]
    fn remove_session() {
        let manager = SessionManager::new(Duration::from_secs(3600), 100);

        let session_id = manager
            .create_session("https://github.com/owner/repo.git", "main", "abc123")
            .unwrap();

        assert!(manager.remove_session(&session_id).unwrap());
        assert!(manager.get_session(&session_id).unwrap().is_none());
    }

    #[test]
    fn session_count() {
        let manager = SessionManager::new(Duration::from_secs(3600), 100);

        assert_eq!(manager.session_count().unwrap(), 0);

        manager
            .create_session("https://github.com/owner/repo1.git", "main", "abc123")
            .unwrap();
        manager
            .create_session("https://github.com/owner/repo2.git", "main", "def456")
            .unwrap();

        assert_eq!(manager.session_count().unwrap(), 2);
    }

    #[test]
    fn list_sessions() {
        let manager = SessionManager::new(Duration::from_secs(3600), 100);

        manager
            .create_session("https://github.com/owner/repo.git", "main", "abc123")
            .unwrap();
        manager
            .create_session("https://github.com/owner/repo.git", "dev", "def456")
            .unwrap();

        let sessions = manager.list_sessions().unwrap();
        assert_eq!(sessions.len(), 2);
    }

    #[test]
    fn update_nonexistent_session_fails() {
        let manager = SessionManager::new(Duration::from_secs(3600), 100);

        let result = manager.update_session_commit("nonexistent", "abc123");
        assert!(result.is_err());
    }

    #[test]
    fn repo_session_age_advances() {
        // Coverage gap fix: `RepoSession::age()` had no caller in any test.
        let session = RepoSession::new(
            "https://github.com/owner/repo.git".to_string(),
            "main".to_string(),
            "abc123".to_string(),
        );

        let first = session.age();
        std::thread::sleep(Duration::from_millis(5));
        let second = session.age();

        // Age is measured from creation, so it only ever grows.
        assert!(second >= first);
        assert!(second >= Duration::from_millis(5));
    }

    #[test]
    fn default_manager_is_empty_and_usable() {
        // Coverage gap fix: `SessionManager::default()` was never constructed.
        let manager = SessionManager::default();
        assert_eq!(manager.session_count().unwrap(), 0);

        // The default (1 hour / 100 sessions) manager still works.
        let id = manager
            .create_session("https://github.com/owner/repo.git", "main", "abc123")
            .unwrap();
        assert!(manager.get_session(&id).unwrap().is_some());
    }

    #[test]
    fn create_session_evicts_expired_to_make_room() {
        // At capacity, expired sessions are evicted so a new session fits.
        let manager = SessionManager::new(Duration::from_millis(1), 1);
        manager
            .create_session("https://github.com/owner/a.git", "main", "abc123")
            .unwrap();

        std::thread::sleep(Duration::from_millis(10));

        // Capacity is 1 and we hold 1 (now-expired) session; the new session
        // must succeed because cleanup evicts the expired one first.
        let id = manager
            .create_session("https://github.com/owner/b.git", "main", "def456")
            .unwrap();
        assert!(manager.get_session(&id).unwrap().is_some());
        assert_eq!(manager.session_count().unwrap(), 1);
    }

    #[test]
    fn create_session_rejects_when_full_of_live_sessions() {
        // At capacity with only live sessions, a new session is rejected.
        let manager = SessionManager::new(Duration::from_secs(3600), 1);
        manager
            .create_session("https://github.com/owner/a.git", "main", "abc123")
            .unwrap();

        let result = manager.create_session("https://github.com/owner/b.git", "main", "def456");
        assert!(
            matches!(result, Err(SessionError::TooManySessions { max }) if max == 1),
            "expected TooManySessions {{ max: 1 }}, got {result:?}"
        );
    }

    #[test]
    fn recreating_existing_session_at_capacity_succeeds() {
        // Regression test: re-creating an *existing* session (same URL +
        // branch) overwrites in place and must not be rejected for capacity,
        // even when the manager is otherwise full. Previously the capacity
        // check ran before the overwrite was recognised, so this returned
        // `TooManySessions`.
        let manager = SessionManager::new(Duration::from_secs(3600), 1);
        let id = manager
            .create_session("https://github.com/owner/a.git", "main", "abc123")
            .unwrap();

        // Same URL + branch -> same session ID -> in-place overwrite.
        let id_again = manager
            .create_session("https://github.com/owner/a.git", "main", "def456")
            .expect("re-creating an existing session at capacity must succeed");

        assert_eq!(id, id_again);
        assert_eq!(manager.session_count().unwrap(), 1);
        let session = manager.get_session(&id).unwrap().unwrap();
        assert_eq!(session.last_commit, "def456");
    }

    #[test]
    fn get_session_removes_and_returns_none_when_expired() {
        // An expired session is dropped (and removed) on access.
        let manager = SessionManager::new(Duration::from_millis(1), 100);
        let id = manager
            .create_session("https://github.com/owner/repo.git", "main", "abc123")
            .unwrap();

        std::thread::sleep(Duration::from_millis(10));

        assert!(manager.get_session(&id).unwrap().is_none());
        // The expired session was removed, not merely hidden.
        assert_eq!(manager.session_count().unwrap(), 0);
    }

    #[test]
    fn cleanup_expired_removes_expired_and_reports_count() {
        // Coverage gap fix: the public `cleanup_expired` method and the
        // `retain`/`info!` body of `cleanup_expired_internal` were untested.
        let manager = SessionManager::new(Duration::from_millis(1), 100);
        manager
            .create_session("https://github.com/owner/a.git", "main", "abc123")
            .unwrap();
        manager
            .create_session("https://github.com/owner/b.git", "main", "def456")
            .unwrap();

        std::thread::sleep(Duration::from_millis(10));

        let removed = manager.cleanup_expired().unwrap();
        assert_eq!(removed, 2);
        assert_eq!(manager.session_count().unwrap(), 0);

        // Idempotent: nothing left to remove.
        assert_eq!(manager.cleanup_expired().unwrap(), 0);
    }

    #[test]
    fn cleanup_expired_keeps_live_sessions() {
        // The retain "keep" arm: a live session survives cleanup.
        let manager = SessionManager::new(Duration::from_secs(3600), 100);
        manager
            .create_session("https://github.com/owner/repo.git", "main", "abc123")
            .unwrap();

        assert_eq!(manager.cleanup_expired().unwrap(), 0);
        assert_eq!(manager.session_count().unwrap(), 1);
    }

    #[test]
    fn session_error_display_messages() {
        // Coverage gap fix: the `Display` impl for every `SessionError`
        // variant was uncovered.
        assert_eq!(
            SessionError::LockPoisoned.to_string(),
            "session lock poisoned"
        );
        assert_eq!(
            SessionError::NotFound("abc@main".to_string()).to_string(),
            "session not found: abc@main"
        );
        assert_eq!(
            SessionError::TooManySessions { max: 7 }.to_string(),
            "too many active sessions (max 7), try again later"
        );
    }

    /// Minimal [`std::io::Write`] that appends to a shared buffer so a test
    /// can capture `tracing` output and assert on it.
    #[derive(Clone)]
    struct CaptureWriter(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for CaptureWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .expect("capture buffer poisoned")
                .extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn create_session_log_uses_sanitised_url() {
        // The `info!` in `create_session` formats the session's *sanitised*
        // URL via `RepoSession::sanitized_url()`. That call is only evaluated
        // when an INFO-level subscriber is active, so install a capturing
        // subscriber and assert the embedded credential never reaches the log.
        let buffer = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = buffer.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::INFO)
            .with_writer(move || CaptureWriter(sink.clone()))
            .finish();

        let manager = SessionManager::new(Duration::from_secs(3600), 100);
        tracing::subscriber::with_default(subscriber, || {
            manager
                .create_session(
                    "https://user:s3cr3t@github.com/owner/repo.git",
                    "main",
                    "abc123",
                )
                .unwrap();
        });

        let logged = String::from_utf8(buffer.lock().unwrap().clone()).unwrap();
        assert!(logged.contains("session created"), "log was: {logged}");
        assert!(
            !logged.contains("s3cr3t"),
            "credential leaked into log: {logged}"
        );
        assert!(logged.contains("github.com"));

        // The fmt layer is line-buffered and may never call the writer's
        // `flush`, so exercise it directly to confirm it is a clean no-op.
        std::io::Write::flush(&mut CaptureWriter(buffer)).unwrap();
    }
}
