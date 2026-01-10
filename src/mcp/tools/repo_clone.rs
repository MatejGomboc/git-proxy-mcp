//! Handler for the `repo/clone` MCP tool.
//!
//! This tool clones a repository and returns its contents as a base64-encoded
//! tar.gz archive. The entire operation happens without writing source files
//! to the user's disk.
//!
//! # Data Flow
//!
//! ```text
//! 1. Fetch to bare repo (temp dir, git objects only)
//! 2. Walk tree, read blobs from object DB
//! 3. Build tar.gz in memory
//! 4. Base64 encode and return
//! 5. Temp dir auto-cleaned
//! ```
//!
//! # Security
//!
//! - Uses credential callbacks (SSH agent, credential helpers)
//! - No source files written to disk
//! - No credentials in response

use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use crate::git2_ops::auth::sanitize_url_for_logging;
use crate::git2_ops::clone::{fetch_bare, FetchOptions2};
use crate::git2_ops::error::Git2Error;
use crate::streaming::tar::{create_tar_from_tree, encode_base64};

/// Arguments for the `repo/clone` tool.
#[derive(Debug, Clone, Deserialize)]
pub struct RepoCloneArgs {
    /// Repository URL (https:// or git@)
    pub url: String,

    /// Branch to clone (defaults to "main")
    #[serde(default)]
    pub branch: Option<String>,

    /// Shallow clone depth (not yet implemented)
    #[serde(default)]
    pub depth: Option<u32>,

    /// Sparse checkout paths (not yet implemented)
    #[serde(default)]
    pub sparse: Option<Vec<String>>,
}

/// Result of a successful `repo/clone` operation.
#[derive(Debug, Clone, Serialize)]
pub struct RepoCloneResult {
    /// Base64-encoded tar.gz archive of the repository
    pub archive: String,

    /// The commit SHA that was cloned
    pub commit: String,

    /// The branch that was cloned
    pub branch: String,

    /// Number of files in the archive
    pub file_count: usize,

    /// Size of the archive in bytes (before base64 encoding)
    pub archive_size: usize,
}

/// Error from repo/clone operation (safe for display).
#[derive(Debug)]
pub struct RepoCloneError {
    /// Error message (credential-safe)
    pub message: String,
}

impl std::fmt::Display for RepoCloneError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl From<Git2Error> for RepoCloneError {
    fn from(err: Git2Error) -> Self {
        Self {
            message: err.to_string(),
        }
    }
}

/// Handle the `repo/clone` tool call.
///
/// This function:
/// 1. Validates the URL
/// 2. Fetches into a bare repository (no working tree)
/// 3. Creates a tar.gz archive from the git tree in memory
/// 4. Returns the base64-encoded archive with metadata
///
/// # Arguments
///
/// - `args`: The tool arguments from the MCP request
///
/// # Returns
///
/// A `RepoCloneResult` with the archive and metadata, or an error.
///
/// # Errors
///
/// Returns `RepoCloneError` if:
/// - URL validation fails
/// - Fetch operation fails (auth, network, etc.)
/// - Tar creation fails
///
/// # Security
///
/// - Credentials are handled via git2 callbacks (never stored)
/// - Source files are never written to disk
/// - The archive is built entirely in memory
pub fn handle_repo_clone(args: RepoCloneArgs) -> Result<RepoCloneResult, RepoCloneError> {
    info!(
        url = %sanitize_url_for_logging(&args.url),
        branch = ?args.branch,
        "repo/clone tool called"
    );

    // Log warnings for unimplemented features
    if args.depth.is_some() {
        debug!("depth parameter not yet implemented, performing full clone");
    }
    if args.sparse.is_some() {
        debug!("sparse parameter not yet implemented, cloning full tree");
    }

    // Fetch into bare repository
    let fetch_opts = FetchOptions2 {
        branch: args.branch,
        depth: args.depth,
    };

    let fetch_result = fetch_bare(&args.url, Some(fetch_opts))?;

    debug!(
        commit = %fetch_result.head_commit,
        branch = %fetch_result.branch,
        "fetch complete, creating tar"
    );

    // Create tar.gz from tree (in memory)
    let tar_result = create_tar_from_tree(&fetch_result.repo, fetch_result.head_commit)?;

    debug!(
        file_count = tar_result.file_count,
        compressed_size = tar_result.data.len(),
        uncompressed_size = tar_result.uncompressed_size,
        "tar creation complete"
    );

    // Base64 encode
    let archive_base64 = encode_base64(&tar_result.data);

    info!(
        commit = %fetch_result.head_commit,
        branch = %fetch_result.branch,
        file_count = tar_result.file_count,
        archive_size = tar_result.data.len(),
        "repo/clone complete"
    );

    Ok(RepoCloneResult {
        archive: archive_base64,
        commit: fetch_result.head_commit.to_string(),
        branch: fetch_result.branch,
        file_count: tar_result.file_count,
        archive_size: tar_result.data.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_clone_args_defaults() {
        let json = r#"{"url": "https://github.com/owner/repo.git"}"#;
        let args: RepoCloneArgs = serde_json::from_str(json).unwrap();
        assert_eq!(args.url, "https://github.com/owner/repo.git");
        assert!(args.branch.is_none());
        assert!(args.depth.is_none());
        assert!(args.sparse.is_none());
    }

    #[test]
    fn repo_clone_args_with_branch() {
        let json = r#"{"url": "https://github.com/owner/repo.git", "branch": "develop"}"#;
        let args: RepoCloneArgs = serde_json::from_str(json).unwrap();
        assert_eq!(args.branch, Some("develop".to_string()));
    }

    #[test]
    fn repo_clone_result_serializes() {
        let result = RepoCloneResult {
            archive: "SGVsbG8=".to_string(),
            commit: "abc123".to_string(),
            branch: "main".to_string(),
            file_count: 10,
            archive_size: 1024,
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"archive\":\"SGVsbG8=\""));
        assert!(json.contains("\"file_count\":10"));
    }

    // Integration tests that require network access are in tests/
}
