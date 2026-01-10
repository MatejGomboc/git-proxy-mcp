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
    let chunk = session_manager.get_chunk(&args.session_id, args.chunk_index)?;

    // Base64 encode the chunk data
    let data_base64 = encode_base64(&chunk.data);

    info!(
        session_id = %args.session_id,
        chunk_index = args.chunk_index,
        chunk_size = chunk.data.len(),
        is_last = chunk.is_last,
        "repo_clone_chunk complete"
    );

    Ok(RepoCloneChunkResult {
        data: data_base64,
        chunk_index: chunk.index,
        chunk_size: chunk.data.len(),
        is_last: chunk.is_last,
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
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"data\":\"SGVsbG8=\""));
        assert!(json.contains("\"is_last\":true"));
    }

    #[test]
    fn repo_clone_cancel_args_deserialize() {
        let json = r#"{"session_id": "stream_xyz"}"#;
        let args: RepoCloneCancelArgs = serde_json::from_str(json).unwrap();
        assert_eq!(args.session_id, "stream_xyz");
    }
}
