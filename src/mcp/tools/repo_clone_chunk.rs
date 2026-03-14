//! Handler for the `repo_clone_chunk` MCP tool (Tier 2).
//!
//! This tool retrieves a chunk from a streaming session created by
//! `repo_clone_start`. The AI calls this repeatedly to get all chunks.
//!
//! # Protocol
//!
//! ```text
//! AI calls repo_clone_chunk with session_id and chunk_index
//! Server returns base64-encoded chunk data and is_last flag
//! AI concatenates chunks client-side to reconstruct tar.gz
//! Session auto-cleans after all chunks retrieved
//! ```
//!
//! # Resume Support
//!
//! Chunks can be requested in any order and multiple times.
//! This allows resuming interrupted transfers.

use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use crate::streaming::chunked::{StreamingError, StreamingSessionManager};
use crate::streaming::tar::encode_base64;

/// Arguments for the `repo_clone_chunk` tool.
#[derive(Debug, Clone, Deserialize)]
pub struct RepoCloneChunkArgs {
    /// Session ID from `repo_clone_start`
    pub session_id: String,

    /// Chunk index to retrieve (0-based)
    pub chunk_index: usize,
}

/// Result of a successful `repo_clone_chunk` operation.
#[derive(Debug, Clone, Serialize)]
pub struct RepoCloneChunkResult {
    /// Base64-encoded chunk data
    pub data: String,

    /// Chunk index (echoed back)
    pub chunk_index: usize,

    /// Size of this chunk in bytes (before base64)
    pub chunk_size: usize,

    /// Whether this is the last chunk
    pub is_last: bool,

    /// Index of the next chunk that has not yet been retrieved (for resume support)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_missing_chunk: Option<usize>,
}

/// Error from `repo_clone_chunk` operation (safe for display).
#[derive(Debug)]
pub struct RepoCloneChunkError {
    /// Error message (credential-safe)
    pub message: String,
}

impl std::fmt::Display for RepoCloneChunkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl From<StreamingError> for RepoCloneChunkError {
    fn from(err: StreamingError) -> Self {
        Self {
            message: err.to_string(),
        }
    }
}

/// Handle the `repo_clone_chunk` tool call.
///
/// This function:
/// 1. Looks up the streaming session
/// 2. Retrieves the requested chunk
/// 3. Base64 encodes the data
/// 4. Returns the chunk with metadata
///
/// # Arguments
///
/// - `args`: The tool arguments from the MCP request
/// - `session_manager`: The streaming session manager
///
/// # Returns
///
/// A `RepoCloneChunkResult` with the chunk data, or an error.
///
/// # Errors
///
/// Returns `RepoCloneChunkError` if:
/// - Session not found (invalid `session_id`)
/// - Session expired (timeout)
/// - Invalid chunk index (out of bounds)
///
/// # Auto-Cleanup
///
/// When the last chunk is retrieved and all chunks have been fetched,
/// the session is automatically cleaned up to free memory.
#[allow(clippy::needless_pass_by_value)] // Consistent with other handlers
pub fn handle_repo_clone_chunk(
    args: RepoCloneChunkArgs,
    session_manager: &StreamingSessionManager,
) -> Result<RepoCloneChunkResult, RepoCloneChunkError> {
    debug!(
        session_id = %args.session_id,
        chunk_index = args.chunk_index,
        "repo_clone_chunk tool called"
    );

    // Get the chunk from the session
    let (chunk, next_missing) = session_manager.get_chunk(&args.session_id, args.chunk_index)?;

    // Base64 encode the chunk data
    let data_base64 = encode_base64(&chunk.data);

    info!(
        session_id = %args.session_id,
        chunk_index = args.chunk_index,
        chunk_size = chunk.data.len(),
        is_last = chunk.is_last,
        next_missing_chunk = ?next_missing,
        "repo_clone_chunk complete"
    );

    Ok(RepoCloneChunkResult {
        data: data_base64,
        chunk_index: chunk.index,
        chunk_size: chunk.data.len(),
        is_last: chunk.is_last,
        next_missing_chunk: next_missing,
    })
}

/// Arguments for the `repo_clone_cancel` tool (optional cleanup).
#[derive(Debug, Clone, Deserialize)]
pub struct RepoCloneCancelArgs {
    /// Session ID to cancel
    pub session_id: String,
}

/// Result of a successful `repo_clone_cancel` operation.
#[derive(Debug, Clone, Serialize)]
pub struct RepoCloneCancelResult {
    /// Whether the session was found and cancelled
    pub cancelled: bool,
}

/// Arguments for the `repo_clone_status` tool.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepoCloneStatusArgs {
    /// Session ID to check status for
    pub session_id: String,
}

/// Result of a successful `repo_clone_status` operation.
#[derive(Debug, Clone, Serialize)]
pub struct RepoCloneStatusResult {
    /// Session ID (echoed back)
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

/// Handle the `repo_clone_status` tool call.
///
/// Returns resume information for a streaming session, enabling
/// the AI to determine which chunks still need to be retrieved
/// after an interruption.
///
/// # Errors
///
/// Returns `RepoCloneChunkError` if:
/// - Session not found (invalid `session_id`)
/// - Session lock is poisoned
#[allow(clippy::needless_pass_by_value)] // Consistent with other handlers
pub fn handle_repo_clone_status(
    args: RepoCloneStatusArgs,
    session_manager: &StreamingSessionManager,
) -> Result<RepoCloneStatusResult, RepoCloneChunkError> {
    info!(
        session_id = %args.session_id,
        "repo_clone_status tool called"
    );

    let status = session_manager.get_session_status(&args.session_id)?;

    Ok(RepoCloneStatusResult {
        session_id: status.session_id,
        total_chunks: status.total_chunks,
        delivered_chunks: status.delivered_chunks,
        next_missing_chunk: status.next_missing_chunk,
        progress_percent: status.progress_percent,
        is_complete: status.is_complete,
    })
}

/// Handle the `repo_clone_cancel` tool call.
///
/// This allows the AI to explicitly cancel a streaming session
/// if it no longer needs the data (e.g., user cancelled the operation).
///
/// Sessions also auto-cleanup after timeout, so this is optional.
///
/// # Errors
///
/// Returns `RepoCloneChunkError` if the session lock is poisoned.
#[allow(clippy::needless_pass_by_value)] // Consistent with other handlers
pub fn handle_repo_clone_cancel(
    args: RepoCloneCancelArgs,
    session_manager: &StreamingSessionManager,
) -> Result<RepoCloneCancelResult, RepoCloneChunkError> {
    info!(
        session_id = %args.session_id,
        "repo_clone_cancel tool called"
    );

    let cancelled = session_manager.cancel_session(&args.session_id)?;

    Ok(RepoCloneCancelResult { cancelled })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_clone_chunk_args_deserialize() {
        let json = r#"{"session_id": "stream_abc", "chunk_index": 5}"#;
        let args: RepoCloneChunkArgs = serde_json::from_str(json).unwrap();
        assert_eq!(args.session_id, "stream_abc");
        assert_eq!(args.chunk_index, 5);
    }

    #[test]
    fn repo_clone_chunk_result_serializes() {
        let result = RepoCloneChunkResult {
            data: "SGVsbG8=".to_string(),
            chunk_index: 2,
            chunk_size: 5,
            is_last: true,
            next_missing_chunk: None,
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"data\":\"SGVsbG8=\""));
        assert!(json.contains("\"is_last\":true"));
        // next_missing_chunk should be skipped when None
        assert!(!json.contains("next_missing_chunk"));
    }

    #[test]
    fn repo_clone_chunk_result_serializes_with_next_missing() {
        let result = RepoCloneChunkResult {
            data: "SGVsbG8=".to_string(),
            chunk_index: 0,
            chunk_size: 5,
            is_last: false,
            next_missing_chunk: Some(1),
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"next_missing_chunk\":1"));
    }

    #[test]
    fn repo_clone_cancel_args_deserialize() {
        let json = r#"{"session_id": "stream_xyz"}"#;
        let args: RepoCloneCancelArgs = serde_json::from_str(json).unwrap();
        assert_eq!(args.session_id, "stream_xyz");
    }

    #[test]
    fn repo_clone_status_args_deserialize() {
        let json = r#"{"session_id": "stream_abc"}"#;
        let args: RepoCloneStatusArgs = serde_json::from_str(json).unwrap();
        assert_eq!(args.session_id, "stream_abc");
    }

    #[test]
    fn repo_clone_status_args_rejects_unknown_fields() {
        let json = r#"{"session_id": "stream_abc", "extra": true}"#;
        let result: Result<RepoCloneStatusArgs, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn repo_clone_status_result_serializes() {
        let result = RepoCloneStatusResult {
            session_id: "stream_abc".to_string(),
            total_chunks: 10,
            delivered_chunks: 3,
            next_missing_chunk: Some(3),
            progress_percent: 30.0,
            is_complete: false,
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"total_chunks\":10"));
        assert!(json.contains("\"delivered_chunks\":3"));
        assert!(json.contains("\"next_missing_chunk\":3"));
        assert!(json.contains("\"is_complete\":false"));
    }

    #[test]
    fn repo_clone_status_result_complete() {
        let result = RepoCloneStatusResult {
            session_id: "stream_abc".to_string(),
            total_chunks: 5,
            delivered_chunks: 5,
            next_missing_chunk: None,
            progress_percent: 100.0,
            is_complete: true,
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"is_complete\":true"));
        assert!(json.contains("\"next_missing_chunk\":null"));
    }
}
