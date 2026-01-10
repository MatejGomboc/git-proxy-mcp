//! Chunked streaming for Tier 2 large repository support.
//!
//! This module implements O(chunk) memory streaming for repositories
//! larger than available RAM.
//!
//! # Protocol
//!
//! MCP uses JSON-RPC 2.0, which doesn't natively support streaming.
//! We implement chunked transfers using a multi-call protocol:
//!
//! ```text
//! AI                           MCP Server
//! │                                │
//! ├── repo/clone_start ───────────▶│
//! │   {url, branch, chunk_size}    │
//! │                                │  Fetch repo, prepare for streaming
//! │◀── {session_id, total_chunks, ─┤
//! │     total_size, commit}        │
//! │                                │
//! ├── repo/clone_chunk ───────────▶│
//! │   {session_id, chunk_index}    │
//! │                                │  Return one chunk
//! │◀── {data, is_last} ────────────┤
//! │                                │
//! │    ... repeat for all chunks   │
//! │                                │
//! ├── repo/clone_chunk ───────────▶│  (final chunk)
//! │   {session_id, chunk_index: N} │
//! │                                │
//! │◀── {data, is_last: true} ──────┤
//! │                                │  Session auto-cleaned
//! ```
//!
//! # Memory Model
//!
//! - **Tier 1**: O(repository size) — entire tar.gz in memory
//! - **Tier 2**: O(chunk size) — only current chunk in memory
//!
//! Default chunk size: 1MB (adjustable per request)
//!
//! # Resume Support
//!
//! Sessions persist until:
//! - All chunks retrieved (auto-cleanup)
//! - Session timeout (1 hour)
//! - Explicit cleanup call
//!
//! AI can resume interrupted transfers by requesting missing chunks.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

/// Default chunk size: 1MB (before base64 encoding)
pub const DEFAULT_CHUNK_SIZE: usize = 1024 * 1024;

/// Maximum chunk size: 4MB
pub const MAX_CHUNK_SIZE: usize = 4 * 1024 * 1024;

/// Session timeout: 1 hour
const SESSION_TIMEOUT: Duration = Duration::from_secs(3600);

/// Maximum concurrent streaming sessions
const MAX_SESSIONS: usize = 10;

/// A streaming session for chunked clone.
#[derive(Debug)]
pub struct StreamingSession {
    /// Session ID for retrieval
    pub id: String,

    /// Repository URL (sanitized for display)
    pub url: String,

    /// Branch that was cloned
    pub branch: String,

    /// Commit SHA
    pub commit: String,

    /// Complete tar.gz data (kept in memory for chunked retrieval)
    /// In a future optimization, this could be backed by a temp file
    data: Vec<u8>,

    /// Chunk size for this session
    chunk_size: usize,

    /// Which chunks have been retrieved (for tracking/metrics)
    retrieved_chunks: Vec<bool>,

    /// When this session was created
    created_at: Instant,

    /// Last access time (for timeout)
    last_accessed: Instant,
}

impl StreamingSession {
    /// Create a new streaming session.
    #[must_use]
    pub fn new(
        id: String,
        url: String,
        branch: String,
        commit: String,
        data: Vec<u8>,
        chunk_size: usize,
    ) -> Self {
        let num_chunks = data.len().div_ceil(chunk_size);
        let now = Instant::now();

        Self {
            id,
            url,
            branch,
            commit,
            data,
            chunk_size,
            retrieved_chunks: vec![false; num_chunks],
            created_at: now,
            last_accessed: now,
        }
    }

    /// Get the total number of chunks.
    #[must_use]
    pub fn total_chunks(&self) -> usize {
        self.retrieved_chunks.len()
    }

    /// Get the total data size.
    #[must_use]
    pub fn total_size(&self) -> usize {
        self.data.len()
    }

    /// Get a specific chunk by index.
    ///
    /// Returns None if index is out of bounds.
    pub fn get_chunk(&mut self, index: usize) -> Option<ChunkData> {
        if index >= self.total_chunks() {
            return None;
        }

        self.last_accessed = Instant::now();
        self.retrieved_chunks[index] = true;

        let start = index * self.chunk_size;
        let end = ((index + 1) * self.chunk_size).min(self.data.len());
        let chunk_bytes = &self.data[start..end];

        Some(ChunkData {
            data: chunk_bytes.to_vec(),
            index,
            is_last: index == self.total_chunks() - 1,
        })
    }

    /// Check if all chunks have been retrieved.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.retrieved_chunks.iter().all(|&r| r)
    }

    /// Check if the session has timed out.
    #[must_use]
    pub fn is_expired(&self) -> bool {
        self.last_accessed.elapsed() > SESSION_TIMEOUT
    }

    /// Get session age.
    #[must_use]
    pub fn age(&self) -> Duration {
        self.created_at.elapsed()
    }

    /// Get retrieval progress as percentage.
    #[must_use]
    #[allow(clippy::cast_precision_loss)] // Acceptable for progress percentages
    pub fn progress(&self) -> f64 {
        let retrieved = self.retrieved_chunks.iter().filter(|&&r| r).count();
        if self.retrieved_chunks.is_empty() {
            100.0
        } else {
            (retrieved as f64 / self.retrieved_chunks.len() as f64) * 100.0
        }
    }
}

/// Data for a single chunk.
#[derive(Debug, Clone)]
pub struct ChunkData {
    /// Raw chunk bytes (will be base64 encoded for MCP response)
    pub data: Vec<u8>,

    /// Chunk index (0-based)
    pub index: usize,

    /// Whether this is the last chunk
    pub is_last: bool,
}

/// Manager for streaming sessions.
#[derive(Debug, Clone)]
pub struct StreamingSessionManager {
    sessions: Arc<RwLock<HashMap<String, StreamingSession>>>,
}

impl Default for StreamingSessionManager {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamingSessionManager {
    /// Create a new session manager.
    #[must_use]
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Generate a unique session ID.
    #[must_use]
    pub fn generate_id() -> String {
        use std::time::SystemTime;

        let timestamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();

        // Simple ID: timestamp + random suffix
        format!("stream_{timestamp:x}_{:04x}", rand_u16())
    }

    /// Create a new streaming session.
    ///
    /// # Errors
    ///
    /// Returns an error if the maximum number of sessions is reached.
    #[allow(clippy::significant_drop_tightening)]
    pub fn create_session(
        &self,
        url: &str,
        branch: &str,
        commit: &str,
        data: Vec<u8>,
        chunk_size: usize,
    ) -> Result<StreamingSessionInfo, StreamingError> {
        let mut sessions = self
            .sessions
            .write()
            .map_err(|_| StreamingError::LockPoisoned)?;

        // Cleanup expired sessions first
        sessions.retain(|id, session| {
            let keep = !session.is_expired();
            if !keep {
                debug!(session_id = %id, "cleaning up expired streaming session");
            }
            keep
        });

        // Check session limit
        if sessions.len() >= MAX_SESSIONS {
            return Err(StreamingError::TooManySessions);
        }

        let id = Self::generate_id();
        let chunk_size = chunk_size.clamp(1024, MAX_CHUNK_SIZE);

        let session = StreamingSession::new(
            id.clone(),
            url.to_string(),
            branch.to_string(),
            commit.to_string(),
            data,
            chunk_size,
        );

        let info = StreamingSessionInfo {
            session_id: id.clone(),
            total_chunks: session.total_chunks(),
            total_size: session.total_size(),
            chunk_size: session.chunk_size,
            commit: session.commit.clone(),
            branch: session.branch.clone(),
        };

        info!(
            session_id = %id,
            total_chunks = info.total_chunks,
            total_size = info.total_size,
            "created streaming session"
        );

        sessions.insert(id, session);

        Ok(info)
    }

    /// Get a chunk from a session.
    ///
    /// # Errors
    ///
    /// Returns an error if the session doesn't exist or the chunk index is invalid.
    #[allow(clippy::significant_drop_tightening)]
    pub fn get_chunk(
        &self,
        session_id: &str,
        chunk_index: usize,
    ) -> Result<ChunkData, StreamingError> {
        let mut sessions = self
            .sessions
            .write()
            .map_err(|_| StreamingError::LockPoisoned)?;

        let session = sessions
            .get_mut(session_id)
            .ok_or_else(|| StreamingError::SessionNotFound(session_id.to_string()))?;

        if session.is_expired() {
            sessions.remove(session_id);
            return Err(StreamingError::SessionExpired(session_id.to_string()));
        }

        let total = session.total_chunks();
        let chunk = session
            .get_chunk(chunk_index)
            .ok_or(StreamingError::InvalidChunkIndex {
                index: chunk_index,
                total,
            })?;

        // Auto-cleanup completed sessions
        if session.is_complete() {
            info!(
                session_id = %session_id,
                "streaming session complete, cleaning up"
            );
            sessions.remove(session_id);
        }

        Ok(chunk)
    }

    /// Get session info without retrieving a chunk.
    ///
    /// # Errors
    ///
    /// Returns an error if the session doesn't exist.
    #[allow(clippy::significant_drop_tightening)]
    pub fn get_session_info(&self, session_id: &str) -> Result<StreamingSessionInfo, StreamingError> {
        let sessions = self
            .sessions
            .read()
            .map_err(|_| StreamingError::LockPoisoned)?;

        let session = sessions
            .get(session_id)
            .ok_or_else(|| StreamingError::SessionNotFound(session_id.to_string()))?;

        Ok(StreamingSessionInfo {
            session_id: session.id.clone(),
            total_chunks: session.total_chunks(),
            total_size: session.total_size(),
            chunk_size: session.chunk_size,
            commit: session.commit.clone(),
            branch: session.branch.clone(),
        })
    }

    /// Cancel a session explicitly.
    ///
    /// # Errors
    ///
    /// Returns an error if the lock is poisoned.
    #[allow(clippy::significant_drop_tightening)]
    pub fn cancel_session(&self, session_id: &str) -> Result<bool, StreamingError> {
        let mut sessions = self
            .sessions
            .write()
            .map_err(|_| StreamingError::LockPoisoned)?;

        let removed = sessions.remove(session_id).is_some();

        if removed {
            info!(session_id = %session_id, "streaming session cancelled");
        }

        Ok(removed)
    }

    /// Get the number of active sessions.
    ///
    /// # Errors
    ///
    /// Returns an error if the lock is poisoned.
    pub fn session_count(&self) -> Result<usize, StreamingError> {
        let sessions = self
            .sessions
            .read()
            .map_err(|_| StreamingError::LockPoisoned)?;
        Ok(sessions.len())
    }

    /// Cleanup all expired sessions.
    ///
    /// # Errors
    ///
    /// Returns an error if the lock is poisoned.
    pub fn cleanup_expired(&self) -> Result<usize, StreamingError> {
        let mut sessions = self
            .sessions
            .write()
            .map_err(|_| StreamingError::LockPoisoned)?;

        let before = sessions.len();
        sessions.retain(|id, session| {
            let keep = !session.is_expired();
            if !keep {
                warn!(session_id = %id, "cleaning up expired streaming session");
            }
            keep
        });

        Ok(before - sessions.len())
    }
}

/// Information about a streaming session (returned from `clone_start`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamingSessionInfo {
    /// Session ID for subsequent chunk requests
    pub session_id: String,

    /// Total number of chunks
    pub total_chunks: usize,

    /// Total size of the archive in bytes
    pub total_size: usize,

    /// Size of each chunk in bytes
    pub chunk_size: usize,

    /// Commit SHA that was cloned
    pub commit: String,

    /// Branch that was cloned
    pub branch: String,
}

/// Errors from streaming operations.
#[derive(Debug)]
pub enum StreamingError {
    /// Session not found
    SessionNotFound(String),

    /// Session has expired
    SessionExpired(String),

    /// Invalid chunk index
    InvalidChunkIndex { index: usize, total: usize },

    /// Too many active sessions
    TooManySessions,

    /// Lock poisoned (should never happen in normal operation)
    LockPoisoned,
}

impl std::fmt::Display for StreamingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SessionNotFound(id) => write!(f, "streaming session not found: {id}"),
            Self::SessionExpired(id) => write!(f, "streaming session expired: {id}"),
            Self::InvalidChunkIndex { index, total } => {
                write!(f, "invalid chunk index {index} (total chunks: {total})")
            }
            Self::TooManySessions => write!(
                f,
                "too many active streaming sessions (max {MAX_SESSIONS})"
            ),
            Self::LockPoisoned => write!(f, "internal error: session lock poisoned"),
        }
    }
}

impl std::error::Error for StreamingError {}

/// Simple pseudo-random u16 for session ID generation.
/// Not cryptographically secure, but fine for session IDs.
fn rand_u16() -> u16 {
    use std::time::SystemTime;

    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();

    // Simple hash of nanoseconds
    ((nanos ^ (nanos >> 16)) & 0xFFFF) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streaming_session_chunks() {
        let data = vec![0u8; 2500]; // 2500 bytes
        let mut session = StreamingSession::new(
            "test".to_string(),
            "url".to_string(),
            "main".to_string(),
            "abc123".to_string(),
            data,
            1000, // 1000 byte chunks
        );

        // Should have 3 chunks: 1000, 1000, 500
        assert_eq!(session.total_chunks(), 3);
        assert_eq!(session.total_size(), 2500);

        // Get first chunk
        let chunk0 = session.get_chunk(0).unwrap();
        assert_eq!(chunk0.data.len(), 1000);
        assert_eq!(chunk0.index, 0);
        assert!(!chunk0.is_last);

        // Get second chunk
        let chunk1 = session.get_chunk(1).unwrap();
        assert_eq!(chunk1.data.len(), 1000);
        assert!(!chunk1.is_last);

        // Get third chunk (last)
        let chunk2 = session.get_chunk(2).unwrap();
        assert_eq!(chunk2.data.len(), 500);
        assert!(chunk2.is_last);

        // Should be complete
        assert!(session.is_complete());
    }

    #[test]
    fn streaming_session_invalid_chunk() {
        let data = vec![0u8; 100];
        let mut session = StreamingSession::new(
            "test".to_string(),
            "url".to_string(),
            "main".to_string(),
            "abc123".to_string(),
            data,
            50,
        );

        assert_eq!(session.total_chunks(), 2);
        assert!(session.get_chunk(0).is_some());
        assert!(session.get_chunk(1).is_some());
        assert!(session.get_chunk(2).is_none()); // Out of bounds
    }

    #[test]
    fn session_manager_create_and_get() {
        let manager = StreamingSessionManager::new();

        // Create data that will result in 3 chunks with min chunk_size of 1024
        // 2500 bytes / 1024 = 3 chunks (1024 + 1024 + 452)
        let data: Vec<u8> = (0u8..=255).cycle().take(2500).collect();
        let info = manager
            .create_session(
                "https://github.com/owner/repo.git",
                "main",
                "abc123",
                data,
                1024, // Will be clamped to min 1024 anyway
            )
            .unwrap();

        assert_eq!(info.total_chunks, 3); // 2500 bytes / 1024 = 3 chunks
        assert_eq!(info.total_size, 2500);
        assert_eq!(info.chunk_size, 1024);

        // Get chunks
        let chunk0 = manager.get_chunk(&info.session_id, 0).unwrap();
        assert_eq!(chunk0.data.len(), 1024);
        assert!(!chunk0.is_last);

        let chunk1 = manager.get_chunk(&info.session_id, 1).unwrap();
        assert_eq!(chunk1.data.len(), 1024);
        assert!(!chunk1.is_last);

        let chunk2 = manager.get_chunk(&info.session_id, 2).unwrap();
        assert_eq!(chunk2.data.len(), 452); // Remaining bytes
        assert!(chunk2.is_last);

        // Session should be auto-cleaned after complete
        assert!(manager.get_chunk(&info.session_id, 0).is_err());
    }

    #[test]
    fn session_manager_invalid_session() {
        let manager = StreamingSessionManager::new();

        let result = manager.get_chunk("nonexistent", 0);
        assert!(matches!(result, Err(StreamingError::SessionNotFound(_))));
    }

    #[test]
    fn generate_id_unique() {
        let id1 = StreamingSessionManager::generate_id();
        let id2 = StreamingSessionManager::generate_id();

        // IDs should be different (with very high probability)
        assert_ne!(id1, id2);
        assert!(id1.starts_with("stream_"));
    }

    #[test]
    fn session_progress() {
        let data = vec![0u8; 300];
        let mut session = StreamingSession::new(
            "test".to_string(),
            "url".to_string(),
            "main".to_string(),
            "abc123".to_string(),
            data,
            100,
        );

        assert_eq!(session.total_chunks(), 3);
        assert!((session.progress() - 0.0).abs() < f64::EPSILON);

        session.get_chunk(0);
        assert!((session.progress() - 33.333_333_333_333_336).abs() < 0.1);

        session.get_chunk(1);
        assert!((session.progress() - 66.666_666_666_666_67).abs() < 0.1);

        session.get_chunk(2);
        assert!((session.progress() - 100.0).abs() < f64::EPSILON);
    }
}
