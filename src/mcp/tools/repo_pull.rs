//! Handler for the `repo_pull` MCP tool.
//!
//! This tool fetches changes since a known commit, providing an incremental
//! update rather than requiring a full re-clone. Useful for keeping an AI's
//! view of a repository up to date efficiently.
//!
//! # Data Flow
//!
//! ```text
//! 1. Fetch branch (with credentials)
//! 2. Generate diff since known commit
//! 3. Create tar.gz of changed/added files
//! 4. Return delta + stats
//! ```
//!
//! # Response Contents
//!
//! - `diff`: Unified diff text (for review)
//! - `files_archive`: Base64 tar.gz of changed/added files (for applying)
//! - `changed_files`: List with paths and change types
//! - `deleted_files`: Files that were removed
//! - `stats`: Commit count, file counts, line counts
//!
//! # Security
//!
//! - Uses credential callbacks (SSH agent, credential helpers)
//! - Temporary bare repo is cleaned up after operation
//! - No source files written to disk
//! - No credentials in response

use serde::{Deserialize, Serialize};
use tracing::info;

use crate::git2_ops::auth::sanitize_url_for_logging;
use crate::git2_ops::error::Git2Error;
use crate::git2_ops::pull::{pull_changes, ChangedFile, PullStats};

/// Arguments for the `repo_pull` tool.
#[derive(Debug, Clone, Deserialize)]
pub struct RepoPullArgs {
    /// Repository URL (https:// or git@)
    pub url: String,

    /// Branch name to sync
    pub branch: String,

    /// Commit SHA that the AI already has (base for delta)
    pub since_commit: String,
}

/// Result of a successful `repo_pull` operation.
#[derive(Debug, Clone, Serialize)]
pub struct RepoPullResult {
    /// Unified diff of all changes (text format)
    pub diff: String,

    /// Base64-encoded tar.gz of changed/added files at HEAD
    pub files_archive: String,

    /// List of changed files with their change types
    pub changed_files: Vec<ChangedFile>,

    /// List of deleted file paths
    pub deleted_files: Vec<String>,

    /// The base commit SHA (what AI had)
    pub base_commit: String,

    /// The new HEAD commit SHA
    pub new_commit: String,

    /// Statistics about the changes
    pub stats: PullStats,

    /// Whether the repository is up to date (no changes)
    pub up_to_date: bool,
}

/// Error from repo_pull operation (safe for display).
#[derive(Debug)]
pub struct RepoPullError {
    /// Error message (credential-safe)
    pub message: String,
}

impl std::fmt::Display for RepoPullError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl From<Git2Error> for RepoPullError {
    fn from(err: Git2Error) -> Self {
        Self {
            message: err.to_string(),
        }
    }
}

/// Handle the `repo_pull` tool call.
///
/// This function:
/// 1. Validates the URL
/// 2. Fetches the branch with credentials
/// 3. Generates a diff since the known commit
/// 4. Creates a tar.gz of changed/added files
/// 5. Returns the delta with full statistics
///
/// # Arguments
///
/// - `args`: The tool arguments from the MCP request
///
/// # Returns
///
/// A `RepoPullResult` with the diff, changed files archive, and stats.
///
/// # Errors
///
/// Returns `RepoPullError` if:
/// - URL validation fails
/// - Fetch fails (auth, network, etc.)
/// - `since_commit` cannot be found
/// - Diff generation fails
///
/// # Security
///
/// - Credentials are handled via git2 callbacks (never stored)
/// - Temporary bare repo is cleaned up after operation
/// - Only diff text, file archive, and metadata are returned
#[allow(clippy::needless_pass_by_value)] // Consistent with other handlers
pub fn handle_repo_pull(args: RepoPullArgs) -> Result<RepoPullResult, RepoPullError> {
    info!(
        url = %sanitize_url_for_logging(&args.url),
        branch = %args.branch,
        since = %args.since_commit,
        "repo_pull tool called"
    );

    let pull_result = pull_changes(&args.url, &args.branch, &args.since_commit)?;

    if pull_result.up_to_date {
        info!("repo_pull: already up to date");
    } else {
        info!(
            commits = pull_result.stats.commits,
            files = pull_result.stats.files_changed,
            added = pull_result.stats.files_added,
            modified = pull_result.stats.files_modified,
            deleted = pull_result.stats.files_deleted,
            "repo_pull complete"
        );
    }

    Ok(RepoPullResult {
        diff: pull_result.diff,
        files_archive: pull_result.files_archive,
        changed_files: pull_result.changed_files,
        deleted_files: pull_result.deleted_files,
        base_commit: pull_result.base_commit,
        new_commit: pull_result.new_commit,
        stats: pull_result.stats,
        up_to_date: pull_result.up_to_date,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_pull_args_parses() {
        let json = r#"{
            "url": "https://github.com/owner/repo.git",
            "branch": "main",
            "since_commit": "abc123def456"
        }"#;
        let args: RepoPullArgs = serde_json::from_str(json).unwrap();
        assert_eq!(args.url, "https://github.com/owner/repo.git");
        assert_eq!(args.branch, "main");
        assert_eq!(args.since_commit, "abc123def456");
    }

    #[test]
    fn repo_pull_result_serializes() {
        let result = RepoPullResult {
            diff: "--- a/file.txt\n+++ b/file.txt\n".to_string(),
            files_archive: "base64data".to_string(),
            changed_files: vec![ChangedFile {
                path: "file.txt".to_string(),
                change_type: "modified".to_string(),
                old_path: None,
            }],
            deleted_files: vec!["old.txt".to_string()],
            base_commit: "abc123".to_string(),
            new_commit: "def456".to_string(),
            stats: PullStats::default(),
            up_to_date: false,
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"diff\":"));
        assert!(json.contains("\"files_archive\":"));
        assert!(json.contains("\"base_commit\":\"abc123\""));
        assert!(json.contains("\"up_to_date\":false"));
    }

    #[test]
    fn repo_pull_error_displays() {
        let err = RepoPullError {
            message: "test error".to_string(),
        };
        assert_eq!(format!("{err}"), "test error");
    }

    // Integration tests that require network access are in tests/
}
