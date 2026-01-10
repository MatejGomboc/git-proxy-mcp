//! Handler for the `repo/clone_start` MCP tool (Tier 2).
//!
//! This tool initiates a chunked clone operation for large repositories.
//! It fetches the repository and creates a streaming session, returning
//! session info that the AI can use to retrieve chunks.
//!
//! # Protocol
//!
//! ```text
//! 1. AI calls repo/clone_start with URL, branch, chunk_size, and options
//! 2. Server fetches repo, creates tar.gz, creates streaming session
//! 3. Server returns session_id, total_chunks, total_size, and statistics
//! 4. AI calls repo/clone_chunk repeatedly to get data
//! ```
//!
//! # Features
//!
//! This tool supports all the same options as `repo/clone`:
//! - Sparse checkout patterns (`sparse`)
//! - Binary file exclusion (`exclude_binary`)
//! - File size limits (`max_file_size`)
//! - LFS resolution (`resolve_lfs`)
//! - Submodule inclusion (`include_submodules`)
//!
//! # Memory Model
//!
//! For archives larger than 10MB, data is stored in a temp file instead
//! of memory (disk-backed storage). The benefits are:
//! - O(chunk size) memory instead of O(archive size)
//! - Progress tracking via chunk retrieval
//! - Resume on failure
//! - Smaller individual responses

use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use crate::git2_ops::auth::sanitize_url_for_logging;
use crate::git2_ops::clone::{fetch_bare, FetchOptions2};
use crate::git2_ops::error::Git2Error;
use crate::streaming::chunked::{
    StreamingError, StreamingSessionManager, DEFAULT_CHUNK_SIZE, MAX_CHUNK_SIZE,
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

    /// Exclude binary files (files with null bytes or mostly non-printable chars).
    /// Useful for AI code review where only source code is needed.
    #[serde(default)]
    pub exclude_binary: Option<bool>,

    /// Maximum file size in bytes. Files larger than this are skipped.
    /// Useful for excluding large generated files or assets.
    #[serde(default)]
    pub max_file_size: Option<usize>,

    /// Resolve Git LFS pointers to actual content.
    /// When enabled, LFS pointer files are replaced with their actual content.
    #[serde(default)]
    pub resolve_lfs: Option<bool>,

    /// Include submodule contents in the archive.
    /// When enabled, submodules are fetched and their files are included
    /// at their respective paths.
    #[serde(default)]
    pub include_submodules: Option<bool>,
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

    /// Number of files in the archive
    pub file_count: usize,

    /// Number of files skipped by sparse filter (if any)
    #[serde(skip_serializing_if = "is_zero")]
    pub skipped_by_filter: usize,

    /// Number of binary files skipped (when `exclude_binary` is true)
    #[serde(skip_serializing_if = "is_zero")]
    pub skipped_binary: usize,

    /// Number of files skipped due to size limit (when `max_file_size` is set)
    #[serde(skip_serializing_if = "is_zero")]
    pub skipped_too_large: usize,

    /// Number of LFS pointers resolved (when `resolve_lfs` is true)
    #[serde(skip_serializing_if = "is_zero")]
    pub lfs_resolved: usize,

    /// Number of LFS pointers that failed to resolve
    #[serde(skip_serializing_if = "is_zero")]
    pub lfs_failed: usize,

    /// Number of submodules successfully included (when `include_submodules` is true)
    #[serde(skip_serializing_if = "is_zero")]
    pub submodules_included: usize,

    /// Number of submodules that failed to fetch
    #[serde(skip_serializing_if = "is_zero")]
    pub submodules_failed: usize,
}

/// Helper for `skip_serializing_if` — skip if value is zero.
#[allow(clippy::trivially_copy_pass_by_ref)] // serde requires &T for skip_serializing_if
#[allow(clippy::missing_const_for_fn)] // serde skip_serializing_if doesn't need const
fn is_zero(n: &usize) -> bool {
    *n == 0
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
#[allow(clippy::too_many_lines)] // Complex setup with many optional features
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
    if args.exclude_binary == Some(true) {
        debug!("binary file exclusion enabled");
    }
    if let Some(max_size) = args.max_file_size {
        debug!(max_size = max_size, "max file size limit set");
    }
    if args.resolve_lfs == Some(true) {
        debug!("LFS resolution enabled");
    }
    if args.include_submodules == Some(true) {
        debug!("submodule inclusion enabled");
    }

    // Determine chunk size
    let chunk_size = args
        .chunk_size
        .map_or(DEFAULT_CHUNK_SIZE, |s| s.min(MAX_CHUNK_SIZE));

    // Fetch into bare repository
    let fetch_opts = FetchOptions2 {
        branch: args.branch.clone(),
        depth: args.depth,
        progress: None,
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
        exclude_binary: args.exclude_binary,
        max_file_size: args.max_file_size,
        resolve_lfs: args.resolve_lfs,
        repo_url: if args.resolve_lfs == Some(true) {
            Some(args.url.clone())
        } else {
            None
        },
        lfs_credentials: None, // TODO: Support LFS credentials from git credential helper
        include_submodules: args.include_submodules,
        progress: None,
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
        skipped_by_filter = tar_result.skipped_by_filter,
        skipped_binary = tar_result.skipped_binary,
        skipped_too_large = tar_result.skipped_too_large,
        lfs_resolved = tar_result.lfs_resolved,
        lfs_failed = tar_result.lfs_failed,
        submodules_included = tar_result.submodules_included,
        submodules_failed = tar_result.submodules_failed,
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
        file_count = tar_result.file_count,
        "repo/clone_start complete"
    );

    Ok(RepoCloneStartResult {
        session_id: session_info.session_id,
        total_chunks: session_info.total_chunks,
        total_size: session_info.total_size,
        chunk_size: session_info.chunk_size,
        commit: session_info.commit,
        branch: session_info.branch,
        file_count: tar_result.file_count,
        skipped_by_filter: tar_result.skipped_by_filter,
        skipped_binary: tar_result.skipped_binary,
        skipped_too_large: tar_result.skipped_too_large,
        lfs_resolved: tar_result.lfs_resolved,
        lfs_failed: tar_result.lfs_failed,
        submodules_included: tar_result.submodules_included,
        submodules_failed: tar_result.submodules_failed,
    })
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
        assert!(args.exclude_binary.is_none());
        assert!(args.max_file_size.is_none());
        assert!(args.resolve_lfs.is_none());
        assert!(args.include_submodules.is_none());
    }

    #[test]
    fn repo_clone_start_args_with_chunk_size() {
        let json = r#"{"url": "https://github.com/owner/repo.git", "chunk_size": 2097152}"#;
        let args: RepoCloneStartArgs = serde_json::from_str(json).unwrap();
        assert_eq!(args.chunk_size, Some(2_097_152));
    }

    #[test]
    fn repo_clone_start_args_with_all_options() {
        let json = r#"{
            "url": "https://github.com/owner/repo.git",
            "branch": "develop",
            "depth": 1,
            "sparse": ["src/**"],
            "chunk_size": 2097152,
            "exclude_binary": true,
            "max_file_size": 1048576,
            "resolve_lfs": true,
            "include_submodules": true
        }"#;
        let args: RepoCloneStartArgs = serde_json::from_str(json).unwrap();
        assert_eq!(args.branch, Some("develop".to_string()));
        assert_eq!(args.depth, Some(1));
        assert_eq!(args.sparse, Some(vec!["src/**".to_string()]));
        assert_eq!(args.chunk_size, Some(2_097_152));
        assert_eq!(args.exclude_binary, Some(true));
        assert_eq!(args.max_file_size, Some(1_048_576));
        assert_eq!(args.resolve_lfs, Some(true));
        assert_eq!(args.include_submodules, Some(true));
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
            file_count: 50,
            skipped_by_filter: 0,
            skipped_binary: 0,
            skipped_too_large: 0,
            lfs_resolved: 0,
            lfs_failed: 0,
            submodules_included: 0,
            submodules_failed: 0,
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"session_id\":\"stream_abc123\""));
        assert!(json.contains("\"total_chunks\":10"));
        assert!(json.contains("\"file_count\":50"));
        // Zero skipped counts should not be serialized
        assert!(!json.contains("skipped"));
        assert!(!json.contains("lfs"));
        assert!(!json.contains("submodules"));
    }

    #[test]
    fn repo_clone_start_result_serializes_skipped_counts() {
        let result = RepoCloneStartResult {
            session_id: "stream_abc123".to_string(),
            total_chunks: 10,
            total_size: 10240,
            chunk_size: 1024,
            commit: "abc123".to_string(),
            branch: "main".to_string(),
            file_count: 50,
            skipped_by_filter: 5,
            skipped_binary: 3,
            skipped_too_large: 2,
            lfs_resolved: 4,
            lfs_failed: 1,
            submodules_included: 2,
            submodules_failed: 1,
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"skipped_by_filter\":5"));
        assert!(json.contains("\"skipped_binary\":3"));
        assert!(json.contains("\"skipped_too_large\":2"));
        assert!(json.contains("\"lfs_resolved\":4"));
        assert!(json.contains("\"lfs_failed\":1"));
        assert!(json.contains("\"submodules_included\":2"));
        assert!(json.contains("\"submodules_failed\":1"));
    }
}
