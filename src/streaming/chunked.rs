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
//! ## Disk-Backed Sessions
//!
//! For archives larger than `DISK_THRESHOLD` (default 10MB), the data is
//! stored in a temporary file instead of memory. This allows handling
//! repositories larger than available RAM while only keeping the current
//! chunk in memory.
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
use std::io::{Read, Seek, SeekFrom, Write};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;
use tracing::{debug, info, warn};

/// Default chunk size: 1MB (before base64 encoding)
pub const DEFAULT_CHUNK_SIZE: usize = 1024 * 1024;

/// Maximum chunk size: 4MB
pub const MAX_CHUNK_SIZE: usize = 4 * 1024 * 1024;

/// Threshold for disk-backed sessions: 10MB
/// Archives larger than this are stored in temp files instead of memory.
pub const DISK_THRESHOLD: usize = 10 * 1024 * 1024;

/// Storage backend for session data.
///
/// Small archives (< `DISK_THRESHOLD`) are kept in memory for fast access.
/// Large archives are stored in temp files to reduce memory pressure.
#[derive(Debug)]
enum SessionStorage {
    /// Data kept in memory (fast, but uses RAM).
    Memory(Vec<u8>),
    /// Data stored in a temp file (slower, but O(chunk) memory).
    /// The file is automatically deleted when the session is dropped.
    File {
        /// The temp file containing the archive data.
        file: NamedTempFile,
        /// Total size of the data (for bounds checking).
        size: usize,
    },
}

impl SessionStorage {
    /// Create storage for the given data.
    ///
    /// If the data is larger than `DISK_THRESHOLD`, it will be written
    /// to a temp file. Otherwise, it stays in memory.
    fn new(data: Vec<u8>) -> Result<Self, std::io::Error> {
        if data.len() > DISK_THRESHOLD {
            // Write to temp file
            let mut file = NamedTempFile::new()?;
            file.write_all(&data)?;
            file.flush()?;
            let size = data.len();
            // Data is dropped here, freeing memory
            Ok(Self::File { file, size })
        } else {
            Ok(Self::Memory(data))
        }
    }

    /// Get the total size of the stored data.
    fn len(&self) -> usize {
        match self {
            Self::Memory(data) => data.len(),
            Self::File { size, .. } => *size,
        }
    }

    /// Read a chunk of data at the specified offset.
    ///
    /// Returns up to `chunk_size` bytes starting at `offset`.
    fn read_chunk(&mut self, offset: usize, chunk_size: usize) -> Result<Vec<u8>, std::io::Error> {
        let total = self.len();
        if offset >= total {
            return Ok(Vec::new());
        }

        let end = (offset + chunk_size).min(total);
        let len = end - offset;

        match self {
            Self::Memory(data) => Ok(data[offset..end].to_vec()),
            Self::File { file, .. } => {
                file.seek(SeekFrom::Start(offset as u64))?;
                let mut buffer = vec![0u8; len];
                file.read_exact(&mut buffer)?;
                Ok(buffer)
            }
        }
    }

    /// Check if storage is disk-backed.
    const fn is_disk_backed(&self) -> bool {
        matches!(self, Self::File { .. })
    }
}

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

    /// Storage backend for archive data.
    /// Small archives are in memory, large ones are disk-backed.
    storage: SessionStorage,

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
    ///
    /// # Errors
    ///
    /// Returns an error if disk-backed storage fails to initialize
    /// (e.g., temp file creation fails).
    pub fn new(
        id: String,
        url: String,
        branch: String,
        commit: String,
        data: Vec<u8>,
        chunk_size: usize,
    ) -> Result<Self, std::io::Error> {
        // Validate chunk_size to prevent division by zero
        if chunk_size == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "chunk_size must be greater than zero",
            ));
        }

        let data_size = data.len();
        let storage = SessionStorage::new(data)?;
        let num_chunks = data_size.div_ceil(chunk_size);
        let now = Instant::now();

        if storage.is_disk_backed() {
            debug!(
                session_id = %id,
                size = data_size,
                "using disk-backed storage for large archive"
            );
        }

        Ok(Self {
            id,
            url,
            branch,
            commit,
            storage,
            chunk_size,
            retrieved_chunks: vec![false; num_chunks],
            created_at: now,
            last_accessed: now,
        })
    }

    /// Check if this session is using disk-backed storage.
    #[must_use]
    pub const fn is_disk_backed(&self) -> bool {
        self.storage.is_disk_backed()
    }

    /// Get the total number of chunks.
    #[must_use]
    pub fn total_chunks(&self) -> usize {
        self.retrieved_chunks.len()
    }

    /// Get the total data size.
    #[must_use]
    pub fn total_size(&self) -> usize {
        self.storage.len()
    }

    /// Get a specific chunk by index.
    ///
    /// Returns None if index is out of bounds or if there's an I/O error
    /// reading from disk-backed storage.
    pub fn get_chunk(&mut self, index: usize) -> Option<ChunkData> {
        if index >= self.total_chunks() {
            return None;
        }

        self.last_accessed = Instant::now();
        self.retrieved_chunks[index] = true;

        let offset = index * self.chunk_size;
        let chunk_bytes = self.storage.read_chunk(offset, self.chunk_size).ok()?;

        Some(ChunkData {
            data: chunk_bytes,
            index,
            is_last: index == self.total_chunks() - 1,
        })
    }

    /// Returns the index of the next chunk that has not yet been retrieved.
    /// Returns `None` if all chunks have been retrieved.
    #[must_use]
    pub fn next_missing_chunk(&self) -> Option<usize> {
        self.retrieved_chunks
            .iter()
            .position(|&retrieved| !retrieved)
    }

    /// Check if all chunks have been retrieved.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.retrieved_chunks.iter().all(|&r| r)
    }

    /// Check if the session has timed out given the configured timeout.
    #[must_use]
    pub fn is_expired(&self, timeout: Duration) -> bool {
        self.last_accessed.elapsed() > timeout
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
    /// Timeout for inactive sessions before automatic cleanup.
    session_timeout: Duration,
    /// Maximum number of concurrent streaming sessions.
    max_sessions: usize,
}

impl Default for StreamingSessionManager {
    fn default() -> Self {
        Self::new(Duration::from_secs(3600), 10)
    }
}

impl StreamingSessionManager {
    /// Create a new session manager with the given timeout and session limit.
    #[must_use]
    pub fn new(session_timeout: Duration, max_sessions: usize) -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            session_timeout,
            max_sessions,
        }
    }

    /// Generate a unique session ID.
    ///
    /// Uses a combination of timestamp and pseudo-random value with a
    /// monotonic counter to ensure uniqueness even under high concurrency.
    #[must_use]
    pub fn generate_id() -> String {
        // rand_u64 includes timestamp, counter, and thread info
        format!("stream_{:016x}", rand_u64())
    }

    /// Create a new streaming session.
    ///
    /// For archives larger than `DISK_THRESHOLD` (10MB), the data is stored
    /// in a temp file instead of memory to reduce memory pressure.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The maximum number of sessions is reached
    /// - Disk-backed storage initialization fails (temp file creation)
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
        let timeout = self.session_timeout;
        sessions.retain(|id, session| {
            let keep = !session.is_expired(timeout);
            if !keep {
                debug!(session_id = %id, "cleaning up expired streaming session");
            }
            keep
        });

        // Check session limit
        if sessions.len() >= self.max_sessions {
            return Err(StreamingError::TooManySessions {
                max: self.max_sessions,
            });
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
        )?;

        let info = StreamingSessionInfo {
            session_id: id.clone(),
            total_chunks: session.total_chunks(),
            total_size: session.total_size(),
            chunk_size: session.chunk_size,
            commit: session.commit.clone(),
            branch: session.branch.clone(),
            delivered_chunks: 0,
            next_missing_chunk: session.next_missing_chunk(),
            progress_percent: session.progress(),
        };

        info!(
            session_id = %id,
            total_chunks = info.total_chunks,
            total_size = info.total_size,
            disk_backed = session.is_disk_backed(),
            "created streaming session"
        );

        sessions.insert(id, session);

        Ok(info)
    }

    /// Get a chunk from a session.
    ///
    /// Returns a tuple of `(ChunkData, Option<usize>)` where the second element
    /// is the index of the next missing chunk (for resume support). This value
    /// is computed before any auto-cleanup of completed sessions.
    ///
    /// # Errors
    ///
    /// Returns an error if the session doesn't exist or the chunk index is invalid.
    #[allow(clippy::significant_drop_tightening)]
    pub fn get_chunk(
        &self,
        session_id: &str,
        chunk_index: usize,
    ) -> Result<(ChunkData, Option<usize>), StreamingError> {
        let mut sessions = self
            .sessions
            .write()
            .map_err(|_| StreamingError::LockPoisoned)?;

        let session = sessions
            .get_mut(session_id)
            .ok_or_else(|| StreamingError::SessionNotFound(session_id.to_string()))?;

        if session.is_expired(self.session_timeout) {
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

        // Compute next missing chunk BEFORE auto-cleanup
        let next_missing = session.next_missing_chunk();

        // Auto-cleanup completed sessions
        if session.is_complete() {
            info!(
                session_id = %session_id,
                "streaming session complete, cleaning up"
            );
            sessions.remove(session_id);
        }

        Ok((chunk, next_missing))
    }

    /// Get session info without retrieving a chunk.
    ///
    /// # Errors
    ///
    /// Returns an error if the session doesn't exist.
    #[allow(clippy::significant_drop_tightening)]
    pub fn get_session_info(
        &self,
        session_id: &str,
    ) -> Result<StreamingSessionInfo, StreamingError> {
        let sessions = self
            .sessions
            .read()
            .map_err(|_| StreamingError::LockPoisoned)?;

        let session = sessions
            .get(session_id)
            .ok_or_else(|| StreamingError::SessionNotFound(session_id.to_string()))?;

        let delivered = session.retrieved_chunks.iter().filter(|&&r| r).count();

        Ok(StreamingSessionInfo {
            session_id: session.id.clone(),
            total_chunks: session.total_chunks(),
            total_size: session.total_size(),
            chunk_size: session.chunk_size,
            commit: session.commit.clone(),
            branch: session.branch.clone(),
            delivered_chunks: delivered,
            next_missing_chunk: session.next_missing_chunk(),
            progress_percent: session.progress(),
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

    /// Get resume information for a session.
    ///
    /// Returns detailed status about which chunks have been delivered,
    /// enabling the AI to resume an interrupted transfer.
    ///
    /// # Errors
    ///
    /// Returns an error if the session doesn't exist or the lock is poisoned.
    #[allow(clippy::significant_drop_tightening)]
    pub fn get_session_status(
        &self,
        session_id: &str,
    ) -> Result<SessionResumeInfo, StreamingError> {
        let sessions = self
            .sessions
            .read()
            .map_err(|_| StreamingError::LockPoisoned)?;

        let session = sessions
            .get(session_id)
            .ok_or_else(|| StreamingError::SessionNotFound(session_id.to_string()))?;

        let delivered = session.retrieved_chunks.iter().filter(|&&r| r).count();

        Ok(SessionResumeInfo {
            session_id: session.id.clone(),
            total_chunks: session.total_chunks(),
            delivered_chunks: delivered,
            next_missing_chunk: session.next_missing_chunk(),
            progress_percent: session.progress(),
            is_complete: session.is_complete(),
        })
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

        let timeout = self.session_timeout;
        let before = sessions.len();
        sessions.retain(|id, session| {
            let keep = !session.is_expired(timeout);
            if !keep {
                warn!(session_id = %id, "cleaning up expired streaming session");
            }
            keep
        });

        Ok(before.saturating_sub(sessions.len()))
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

    /// Number of chunks successfully delivered so far
    pub delivered_chunks: usize,

    /// Index of the next chunk that has not yet been retrieved
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_missing_chunk: Option<usize>,

    /// Progress as a percentage (0.0 to 100.0)
    pub progress_percent: f64,
}

/// Resume information for a streaming session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionResumeInfo {
    /// Session ID
    pub session_id: String,

    /// Total number of chunks
    pub total_chunks: usize,

    /// Number of chunks successfully delivered so far
    pub delivered_chunks: usize,

    /// Index of the next chunk that has not yet been retrieved
    pub next_missing_chunk: Option<usize>,

    /// Progress as a percentage (0.0 to 100.0)
    pub progress_percent: f64,

    /// Whether all chunks have been retrieved
    pub is_complete: bool,
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
    TooManySessions { max: usize },

    /// Lock poisoned (should never happen in normal operation)
    LockPoisoned,

    /// I/O error (e.g., temp file creation failed)
    IoError(String),
}

impl std::fmt::Display for StreamingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SessionNotFound(id) => write!(f, "streaming session not found: {id}"),
            Self::SessionExpired(id) => write!(f, "streaming session expired: {id}"),
            Self::InvalidChunkIndex { index, total } => {
                write!(f, "invalid chunk index {index} (total chunks: {total})")
            }
            Self::TooManySessions { max } => {
                write!(f, "too many active streaming sessions (max {max})")
            }
            Self::LockPoisoned => write!(f, "internal error: session lock poisoned"),
            Self::IoError(msg) => write!(f, "I/O error: {msg}"),
        }
    }
}

impl From<std::io::Error> for StreamingError {
    fn from(err: std::io::Error) -> Self {
        Self::IoError(err.to_string())
    }
}

impl std::error::Error for StreamingError {}

/// Simple pseudo-random u64 for session ID generation.
/// Not cryptographically secure, but provides sufficient uniqueness for session IDs.
/// Uses multiple entropy sources to reduce collision probability.
#[allow(clippy::cast_possible_truncation)] // Truncation is intentional for mixing
fn rand_u64() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::SystemTime;

    // Monotonic counter to ensure uniqueness even within same nanosecond
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);

    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;

    // Mix counter, nanoseconds, and thread ID for better entropy
    let thread_id = std::thread::current().id();
    let thread_hash = format!("{thread_id:?}").len() as u64;

    // Simple mixing function using large primes
    let mixed = nanos
        .wrapping_mul(0x517c_c1b7_2722_0a95)
        .wrapping_add(counter)
        .wrapping_mul(0x2545_f491_4f6c_dd1d)
        .wrapping_add(thread_hash);

    mixed ^ (mixed >> 32)
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
        )
        .unwrap();

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
        )
        .unwrap();

        assert_eq!(session.total_chunks(), 2);
        assert!(session.get_chunk(0).is_some());
        assert!(session.get_chunk(1).is_some());
        assert!(session.get_chunk(2).is_none()); // Out of bounds
    }

    #[test]
    fn session_manager_create_and_get() {
        let manager = StreamingSessionManager::default();

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
        let (chunk0, _) = manager.get_chunk(&info.session_id, 0).unwrap();
        assert_eq!(chunk0.data.len(), 1024);
        assert!(!chunk0.is_last);

        let (chunk1, _) = manager.get_chunk(&info.session_id, 1).unwrap();
        assert_eq!(chunk1.data.len(), 1024);
        assert!(!chunk1.is_last);

        let (chunk2, _) = manager.get_chunk(&info.session_id, 2).unwrap();
        assert_eq!(chunk2.data.len(), 452); // Remaining bytes
        assert!(chunk2.is_last);

        // Session should be auto-cleaned after complete
        assert!(manager.get_chunk(&info.session_id, 0).is_err());
    }

    #[test]
    fn session_manager_invalid_session() {
        let manager = StreamingSessionManager::default();

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
        )
        .unwrap();

        assert_eq!(session.total_chunks(), 3);
        assert!((session.progress() - 0.0).abs() < f64::EPSILON);

        session.get_chunk(0);
        assert!((session.progress() - 33.333_333_333_333_336).abs() < 0.1);

        session.get_chunk(1);
        assert!((session.progress() - 66.666_666_666_666_67).abs() < 0.1);

        session.get_chunk(2);
        assert!((session.progress() - 100.0).abs() < f64::EPSILON);
    }

    #[test]
    fn streaming_session_rejects_zero_chunk_size() {
        let data = vec![0u8; 100];
        let result = StreamingSession::new(
            "test".to_string(),
            "url".to_string(),
            "main".to_string(),
            "abc123".to_string(),
            data,
            0, // Zero chunk_size should fail
        );

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn next_missing_chunk_tracks_retrieval() {
        let data = vec![0u8; 300];
        let mut session = StreamingSession::new(
            "test".to_string(),
            "url".to_string(),
            "main".to_string(),
            "abc123".to_string(),
            data,
            100,
        )
        .unwrap();

        // Initially, the first missing chunk is 0
        assert_eq!(session.next_missing_chunk(), Some(0));

        // Retrieve chunk 0 — next missing should be 1
        session.get_chunk(0);
        assert_eq!(session.next_missing_chunk(), Some(1));

        // Retrieve chunk 2 (out of order) — next missing should still be 1
        session.get_chunk(2);
        assert_eq!(session.next_missing_chunk(), Some(1));

        // Retrieve chunk 1 — all retrieved, next missing should be None
        session.get_chunk(1);
        assert_eq!(session.next_missing_chunk(), None);
    }

    #[test]
    fn get_chunk_returns_next_missing() {
        let manager = StreamingSessionManager::default();

        let data: Vec<u8> = (0u8..=255).cycle().take(3072).collect();
        let info = manager
            .create_session(
                "https://github.com/owner/repo.git",
                "main",
                "abc123",
                data,
                1024,
            )
            .unwrap();

        assert_eq!(info.total_chunks, 3);

        // Get chunk 0 — next missing should be 1
        let (_, next_missing) = manager.get_chunk(&info.session_id, 0).unwrap();
        assert_eq!(next_missing, Some(1));

        // Get chunk 1 — next missing should be 2
        let (_, next_missing) = manager.get_chunk(&info.session_id, 1).unwrap();
        assert_eq!(next_missing, Some(2));

        // Get chunk 2 (last) — next missing should be None (all retrieved)
        let (_, next_missing) = manager.get_chunk(&info.session_id, 2).unwrap();
        assert_eq!(next_missing, None);
    }

    #[test]
    fn get_session_status_returns_resume_info() {
        let manager = StreamingSessionManager::default();

        let data: Vec<u8> = (0u8..=255).cycle().take(3072).collect();
        let info = manager
            .create_session(
                "https://github.com/owner/repo.git",
                "main",
                "abc123",
                data,
                1024,
            )
            .unwrap();

        // Check initial status
        let status = manager.get_session_status(&info.session_id).unwrap();
        assert_eq!(status.total_chunks, 3);
        assert_eq!(status.delivered_chunks, 0);
        assert_eq!(status.next_missing_chunk, Some(0));
        assert!((status.progress_percent - 0.0).abs() < f64::EPSILON);
        assert!(!status.is_complete);

        // Retrieve chunk 0
        manager.get_chunk(&info.session_id, 0).unwrap();

        let status = manager.get_session_status(&info.session_id).unwrap();
        assert_eq!(status.delivered_chunks, 1);
        assert_eq!(status.next_missing_chunk, Some(1));
        assert!((status.progress_percent - 33.333_333_333_333_336).abs() < 0.1);
        assert!(!status.is_complete);

        // Retrieve chunk 1
        manager.get_chunk(&info.session_id, 1).unwrap();

        let status = manager.get_session_status(&info.session_id).unwrap();
        assert_eq!(status.delivered_chunks, 2);
        assert_eq!(status.next_missing_chunk, Some(2));
        assert!(!status.is_complete);
    }

    #[test]
    fn get_session_status_not_found() {
        let manager = StreamingSessionManager::default();
        let result = manager.get_session_status("nonexistent");
        assert!(matches!(result, Err(StreamingError::SessionNotFound(_))));
    }

    #[test]
    fn session_info_has_resume_fields() {
        let manager = StreamingSessionManager::default();

        let data: Vec<u8> = (0u8..=255).cycle().take(2048).collect();
        let info = manager
            .create_session(
                "https://github.com/owner/repo.git",
                "main",
                "abc123",
                data,
                1024,
            )
            .unwrap();

        // New session should have resume fields initialised
        assert_eq!(info.delivered_chunks, 0);
        assert_eq!(info.next_missing_chunk, Some(0));
        assert!((info.progress_percent - 0.0).abs() < f64::EPSILON);
    }
}
