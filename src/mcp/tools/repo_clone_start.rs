//! Handler for the `repo/clone_start` MCP tool (Tier 2).
//!
//! This tool initiates a chunked clone operation for large repositories.
//! It fetches the repository and creates a streaming session, returning
//! session info that the AI can use to retrieve chunks.
//!
//! # Protocol
//!
//! ```text
//! 1. AI calls repo/clone_start with URL, branch, chunk_size
//! 2. Server fetches repo, creates tar.gz, creates streaming session
//! 3. Server returns session_id, total_chunks, total_size
//! 4. AI calls repo/clone_chunk repeatedly to get data
//! ```
//!
//! # Memory Model
//!
//! The entire archive is still buffered in memory on the server side.
//! The benefit is that the AI can retrieve it in chunks, allowing for:
//! - Progress tracking
//! - Resume on failure
//! - Smaller individual responses

use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use crate::git2_ops::auth::sanitize_url_for_logging;
use crate::git2_ops::clone::{fetch_bare, FetchOptions2};
use crate::git2_ops::error::Git2Error;
use crate::streaming::chunked::{
    StreamingError, StreamingSessionInfo, StreamingSessionManager, DEFAULT_CHUNK_SIZE,
    MAX_CHUNK_SIZE,
};
use crate::streaming::tar::{create_tar_from_tree_with_options, TarOptions};

/// Arguments for the `repo/clone_start` tool.
#[derive(Debug, Clone, Deserialize)]
pub struct RepoCloneStartArgs {
    /// Repository URL (https:// or git@)
    pub url: String,

    /// Branch to clone (defaults to "main")
    #[serde(default)]
    pub branch: Option<String>,

    /// Shallow clone depth (1 = only latest commit, None = full history)
    #[serde(default)]
    pub depth: Option<u32>,

    /// Sparse checkout paths — only include files matching these patterns
    #[serde(default)]
    pub sparse: Option<Vec<String>>,

    /// Chunk size in bytes (default: 1MB, max: 4MB)
    #[serde(default)]
    pub chunk_size: Option<usize>,
}

/// Result of a successful `repo/clone_start` operation.
#[derive(Debug, Clone, Serialize)]
pub struct RepoCloneStartResult {
    /// Session ID for subsequent chunk requests
    pub session_id: String,

    /// Total number of chunks to retrieve
    pub total_chunks: usize,

    /// Total size of the archive in bytes
    pub total_size: usize,

    /// Size of each chunk in bytes
    pub chunk_size: usize,

    /// The commit SHA that was cloned
    pub commit: String,

    /// The branch that was cloned
    pub branch: String,
}

impl From<StreamingSessionInfo> for RepoCloneStartResult {
    fn from(info: StreamingSessionInfo) -> Self {
        Self {
            session_id: info.session_id,
            total_chunks: info.total_chunks,
            total_size: info.total_size,
            chunk_size: info.chunk_size,
            commit: info.commit,
            branch: info.branch,
        }
    }
}

/// Error from `repo/clone_start` operation (safe for display).
#[derive(Debug)]
pub struct RepoCloneStartError {
    /// Error message (credential-safe)
    pub message: String,
}

impl std::fmt::Display for RepoCloneStartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl From<Git2Error> for RepoCloneStartError {
    fn from(err: Git2Error) -> Self {
        Self {
            message: err.to_string(),
        }
    }
}

impl From<StreamingError> for RepoCloneStartError {
    fn from(err: StreamingError) -> Self {
        Self {
            message: err.to_string(),
        }
    }
}

/// Handle the `repo/clone_start` tool call.
///
/// This function:
/// 1. Validates the URL
/// 2. Fetches into a bare repository (no working tree)
/// 3. Creates a tar.gz archive from the git tree in memory
/// 4. Creates a streaming session for chunked retrieval
/// 5. Returns session info for chunk requests
///
/// # Arguments
///
/// - `args`: The tool arguments from the MCP request
/// - `session_manager`: The streaming session manager
///
/// # Returns
///
/// A `RepoCloneStartResult` with session info, or an error.
///
/// # Errors
///
/// Returns `RepoCloneStartError` if:
/// - URL validation fails
/// - Fetch operation fails (auth, network, etc.)
/// - Tar creation fails
/// - Session creation fails (too many active sessions)
pub fn handle_repo_clone_start(
    args: RepoCloneStartArgs,
    session_manager: &StreamingSessionManager,
) -> Result<RepoCloneStartResult, RepoCloneStartError> {
    let sanitized_url = sanitize_url_for_logging(&args.url);

    info!(
        url = %sanitized_url,
        branch = ?args.branch,
        chunk_size = ?args.chunk_size,
        "repo/clone_start tool called"
    );

    // Log info about optional features
    if let Some(depth) = args.depth {
        debug!(depth = depth, "shallow clone requested");
    }
    if let Some(ref sparse) = args.sparse {
        debug!(patterns = ?sparse, "sparse checkout requested");
    }

    // Determine chunk size
    let chunk_size = args
        .chunk_size
        .map_or(DEFAULT_CHUNK_SIZE, |s| s.min(MAX_CHUNK_SIZE));

    // Fetch into bare repository
    let fetch_opts = FetchOptions2 {
        branch: args.branch.clone(),
        depth: args.depth,
        progress: None, // TODO: Add progress support to chunked streaming
    };

    let fetch_result = fetch_bare(&args.url, Some(fetch_opts))?;

    debug!(
        commit = %fetch_result.head_commit,
        branch = %fetch_result.branch,
        "fetch complete, creating tar"
    );

    // Create tar.gz from tree (in memory), with optional filtering
    let tar_opts = TarOptions {
        sparse_patterns: args.sparse,
        exclude_binary: None,       // TODO: Add to RepoCloneStartArgs when needed
        max_file_size: None,        // TODO: Add to RepoCloneStartArgs when needed
        resolve_lfs: None,          // TODO: Add to RepoCloneStartArgs when needed
        repo_url: None,             // TODO: Add to RepoCloneStartArgs when needed
        lfs_credentials: None,      // TODO: Add to RepoCloneStartArgs when needed
        include_submodules: None,   // TODO: Add to RepoCloneStartArgs when needed
        progress: None,             // TODO: Add progress support to chunked streaming
    };

    let tar_result = create_tar_from_tree_with_options(
        &fetch_result.repo,
        fetch_result.head_commit,
        Some(tar_opts),
    )?;

    debug!(
        file_count = tar_result.file_count,
        compressed_size = tar_result.data.len(),
        uncompressed_size = tar_result.uncompressed_size,
        "tar creation complete, creating streaming session"
    );

    // Create streaming session
    let session_info = session_manager.create_session(
        &sanitized_url,
        &fetch_result.branch,
        &fetch_result.head_commit.to_string(),
        tar_result.data,
        chunk_size,
    )?;

    info!(
        session_id = %session_info.session_id,
        total_chunks = session_info.total_chunks,
        total_size = session_info.total_size,
        chunk_size = session_info.chunk_size,
        "repo/clone_start complete"
    );

    Ok(session_info.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_clone_start_args_defaults() {
        let json = r#"{"url": "https://github.com/owner/repo.git"}"#;
        let args: RepoCloneStartArgs = serde_json::from_str(json).unwrap();
        assert_eq!(args.url, "https://github.com/owner/repo.git");
        assert!(args.branch.is_none());
        assert!(args.depth.is_none());
        assert!(args.sparse.is_none());
        assert!(args.chunk_size.is_none());
    }

    #[test]
    fn repo_clone_start_args_with_chunk_size() {
        let json = r#"{"url": "https://github.com/owner/repo.git", "chunk_size": 2097152}"#;
        let args: RepoCloneStartArgs = serde_json::from_str(json).unwrap();
        assert_eq!(args.chunk_size, Some(2_097_152));
    }

    #[test]
    fn repo_clone_start_result_serializes() {
        let result = RepoCloneStartResult {
            session_id: "stream_abc123".to_string(),
            total_chunks: 10,
            total_size: 10240,
            chunk_size: 1024,
            commit: "abc123".to_string(),
            branch: "main".to_string(),
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"session_id\":\"stream_abc123\""));
        assert!(json.contains("\"total_chunks\":10"));
    }
}
