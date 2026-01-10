//! Handler for the `repo_refs` MCP tool.
//!
//! This tool lists branches and tags from a remote repository without cloning.
//! It's equivalent to `git ls-remote` but with structured output.
//!
//! # Data Flow
//!
//! ```text
//! 1. Connect to remote (with credentials)
//! 2. List refs (branch/tag names + OIDs)
//! 3. Return structured data
//! ```
//!
//! # Security
//!
//! - Uses credential callbacks (SSH agent, credential helpers)
//! - No repository data is downloaded
//! - No files are written to disk
//! - No credentials in response

use serde::{Deserialize, Serialize};
use tracing::info;

use crate::git2_ops::auth::sanitize_url_for_logging;
use crate::git2_ops::error::Git2Error;
use crate::git2_ops::refs::{list_remote_refs, RefInfo};

/// Arguments for the `repo_refs` tool.
#[derive(Debug, Clone, Deserialize)]
pub struct RepoRefsArgs {
    /// Repository URL (https:// or git@)
    pub url: String,
}

/// Result of a successful `repo_refs` operation.
#[derive(Debug, Clone, Serialize)]
pub struct RepoRefsResult {
    /// List of branches with their commit SHAs
    pub branches: Vec<RefInfo>,

    /// List of tags with their commit SHAs
    pub tags: Vec<RefInfo>,

    /// Default branch name (e.g., "main", "master")
    pub default_branch: String,

    /// Total number of references (branches + tags)
    pub total_refs: usize,
}

/// Error from `repo_refs` operation (safe for display).
#[derive(Debug)]
pub struct RepoRefsError {
    /// Error message (credential-safe)
    pub message: String,
}

impl std::fmt::Display for RepoRefsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl From<Git2Error> for RepoRefsError {
    fn from(err: Git2Error) -> Self {
        Self {
            message: err.to_string(),
        }
    }
}

/// Handle the `repo_refs` tool call.
///
/// This function:
/// 1. Validates the URL
/// 2. Connects to the remote with credentials
/// 3. Lists all branches and tags
/// 4. Returns structured reference information
///
/// # Arguments
///
/// - `args`: The tool arguments from the MCP request
///
/// # Returns
///
/// A `RepoRefsResult` with branches, tags, and default branch info.
///
/// # Errors
///
/// Returns `RepoRefsError` if:
/// - URL validation fails
/// - Connection fails (auth, network, etc.)
///
/// # Security
///
/// - Credentials are handled via git2 callbacks (never stored)
/// - No repository data is downloaded
/// - Only ref names and commit SHAs are returned
#[allow(clippy::needless_pass_by_value)] // Consistent with other handlers
pub fn handle_repo_refs(args: RepoRefsArgs) -> Result<RepoRefsResult, RepoRefsError> {
    info!(
        url = %sanitize_url_for_logging(&args.url),
        "repo_refs tool called"
    );

    let refs_result = list_remote_refs(&args.url)?;

    info!(
        branches = refs_result.branches.len(),
        tags = refs_result.tags.len(),
        default_branch = %refs_result.default_branch,
        "repo_refs complete"
    );

    Ok(RepoRefsResult {
        branches: refs_result.branches,
        tags: refs_result.tags,
        default_branch: refs_result.default_branch,
        total_refs: refs_result.total_refs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_refs_args_parses() {
        let json = r#"{"url": "https://github.com/owner/repo.git"}"#;
        let args: RepoRefsArgs = serde_json::from_str(json).unwrap();
        assert_eq!(args.url, "https://github.com/owner/repo.git");
    }

    #[test]
    fn repo_refs_result_serializes() {
        let result = RepoRefsResult {
            branches: vec![RefInfo {
                name: "refs/heads/main".to_string(),
                short_name: "main".to_string(),
                commit: "abc123def456".to_string(),
            }],
            tags: vec![RefInfo {
                name: "refs/tags/v1.0.0".to_string(),
                short_name: "v1.0.0".to_string(),
                commit: "789xyz".to_string(),
            }],
            default_branch: "main".to_string(),
            total_refs: 2,
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"branches\""));
        assert!(json.contains("\"tags\""));
        assert!(json.contains("\"default_branch\":\"main\""));
        assert!(json.contains("\"total_refs\":2"));
    }

    #[test]
    fn repo_refs_error_displays() {
        let err = RepoRefsError {
            message: "test error".to_string(),
        };
        assert_eq!(format!("{err}"), "test error");
    }

    // Integration tests that require network access are in tests/
}
