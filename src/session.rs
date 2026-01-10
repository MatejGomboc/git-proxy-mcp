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

/// Maximum session age before automatic cleanup (1 hour).
const SESSION_MAX_AGE: Duration = Duration::from_secs(3600);

/// Maximum number of sessions to prevent memory exhaustion.
const MAX_SESSIONS: usize = 100;

/// A session tracking a cloned repository.
#[derive(Debug, Clone)]
pub struct RepoSession {
    /// Repository URL (sanitized for display, original for operations).
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

    /// Get the repository URL sanitized for logging/display.
    #[must_use]
    pub fn sanitized_url(&self) -> String {
        sanitize_url_for_logging(&self.url)
    }

    /// Check if this session has expired.
    #[must_use]
    pub fn is_expired(&self) -> bool {
        self.last_accessed.elapsed() > SESSION_MAX_AGE
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
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionManager {
    /// Create a new session manager.
    #[must_use]
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Generate a session ID from URL and branch.
    #[must_use]
    pub fn session_id(url: &str, branch: &str) -> String {
        // Use sanitized URL for the key to avoid storing credentials
        let sanitized = sanitize_url_for_logging(url);
        format!("{sanitized}@{branch}")
    }

    /// Create or update a session after a clone operation.
    ///
    /// # Errors
    ///
    /// Returns an error if the session limit is reached and cleanup fails.
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

        // Clean up expired sessions if we're at capacity
        if sessions.len() >= MAX_SESSIONS {
            self.cleanup_expired_internal(&mut sessions);

            // Still at capacity after cleanup?
            if sessions.len() >= MAX_SESSIONS {
                return Err(SessionError::TooManySessions);
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
    pub fn get_session(&self, session_id: &str) -> Result<Option<RepoSession>, SessionError> {
        let mut sessions = self
            .sessions
            .write()
            .map_err(|_| SessionError::LockPoisoned)?;

        if let Some(session) = sessions.get_mut(session_id) {
            if session.is_expired() {
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
        let before = sessions.len();
        sessions.retain(|id, session| {
            let keep = !session.is_expired();
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
    TooManySessions,
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LockPoisoned => write!(f, "session lock poisoned"),
            Self::NotFound(id) => write!(f, "session not found: {id}"),
            Self::TooManySessions => write!(
                f,
                "too many active sessions (max {MAX_SESSIONS}), try again later"
            ),
        }
    }
}

impl std::error::Error for SessionError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_and_get_session() {
        let manager = SessionManager::new();

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
        let id = SessionManager::session_id(
            "https://user:secret@github.com/owner/repo.git",
            "main",
        );

        // Should not contain the password
        assert!(!id.contains("secret"));
        assert!(id.contains("github.com"));
        assert!(id.contains("main"));
    }

    #[test]
    fn update_session_commit() {
        let manager = SessionManager::new();

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
        let manager = SessionManager::new();

        let session_id = manager
            .create_session("https://github.com/owner/repo.git", "main", "abc123")
            .unwrap();

        assert!(manager.remove_session(&session_id).unwrap());
        assert!(manager.get_session(&session_id).unwrap().is_none());
    }

    #[test]
    fn session_count() {
        let manager = SessionManager::new();

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
        let manager = SessionManager::new();

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
        let manager = SessionManager::new();

        let result = manager.update_session_commit("nonexistent", "abc123");
        assert!(result.is_err());
    }
}
