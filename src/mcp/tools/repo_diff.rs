//! Handler for the `repo_diff` MCP tool.
//!
//! This tool generates a unified diff between two commits from a remote
//! repository. It's useful for reviewing changes without downloading the
//! entire repository content.
//!
//! # Data Flow
//!
//! ```text
//! 1. Fetch repository (with credentials)
//! 2. Resolve both commit references
//! 3. Generate diff between trees
//! 4. Return unified diff + stats
//! ```
//!
//! # Security
//!
//! - Uses credential callbacks (SSH agent, credential helpers)
//! - Temporary bare repo is cleaned up after operation
//! - No source files written to disk
//! - No credentials in response

use serde::{Deserialize, Serialize};
use tracing::info;

use crate::config::ProxyConfig;
use crate::git2_ops::auth::sanitize_url_for_logging;
use crate::git2_ops::diff::{generate_diff, DiffStats};
use crate::git2_ops::error::Git2Error;

/// Arguments for the `repo_diff` tool.
#[derive(Debug, Clone, Deserialize)]
pub struct RepoDiffArgs {
    /// Repository URL (https:// or git@)
    pub url: String,

    /// Base commit reference (SHA, branch name, tag, or relative ref like HEAD~5)
    pub base_commit: String,

    /// Head commit reference (SHA, branch name, tag, or relative ref)
    pub head_commit: String,
}

/// Result of a successful `repo_diff` operation.
#[derive(Debug, Clone, Serialize)]
pub struct RepoDiffResult {
    /// Unified diff output
    pub diff: String,

    /// Diff statistics
    pub stats: DiffStats,

    /// Resolved base commit SHA (full 40-char)
    pub base_commit: String,

    /// Resolved head commit SHA (full 40-char)
    pub head_commit: String,
}

/// Error from `repo_diff` operation (safe for display).
#[derive(Debug)]
pub struct RepoDiffError {
    /// Error message (credential-safe)
    pub message: String,
}

impl std::fmt::Display for RepoDiffError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl From<Git2Error> for RepoDiffError {
    fn from(err: Git2Error) -> Self {
        Self {
            message: err.to_string(),
        }
    }
}

/// Handle the `repo_diff` tool call.
///
/// This function:
/// 1. Validates the URL
/// 2. Fetches the repository with credentials
/// 3. Resolves both commit references
/// 4. Generates a unified diff between the commits
/// 5. Returns the diff and statistics
///
/// # Arguments
///
/// - `args`: The tool arguments from the MCP request
///
/// # Returns
///
/// A `RepoDiffResult` with the unified diff, stats, and resolved commit SHAs.
///
/// # Errors
///
/// Returns `RepoDiffError` if:
/// - URL validation fails
/// - Fetch fails (auth, network, etc.)
/// - Either commit cannot be resolved
/// - Diff generation fails
///
/// # Security
///
/// - Credentials are handled via git2 callbacks (never stored)
/// - Temporary bare repo is cleaned up after operation
/// - Only diff text and metadata are returned
#[allow(clippy::needless_pass_by_value)] // Consistent with other handlers
pub fn handle_repo_diff(
    args: RepoDiffArgs,
    proxy_config: &ProxyConfig,
) -> Result<RepoDiffResult, RepoDiffError> {
    info!(
        url = %sanitize_url_for_logging(&args.url),
        base = %args.base_commit,
        head = %args.head_commit,
        "repo_diff tool called"
    );

    let diff_result = generate_diff(
        &args.url,
        &args.base_commit,
        &args.head_commit,
        proxy_config.url.as_deref(),
    )?;

    info!(
        files = diff_result.stats.files_changed,
        insertions = diff_result.stats.insertions,
        deletions = diff_result.stats.deletions,
        "repo_diff complete"
    );

    Ok(RepoDiffResult {
        diff: diff_result.diff,
        stats: diff_result.stats,
        base_commit: diff_result.base_commit,
        head_commit: diff_result.head_commit,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_diff_args_parses() {
        let json = r#"{
            "url": "https://github.com/owner/repo.git",
            "base_commit": "abc123",
            "head_commit": "def456"
        }"#;
        let args: RepoDiffArgs = serde_json::from_str(json).unwrap();
        assert_eq!(args.url, "https://github.com/owner/repo.git");
        assert_eq!(args.base_commit, "abc123");
        assert_eq!(args.head_commit, "def456");
    }

    #[test]
    fn repo_diff_result_serializes() {
        let result = RepoDiffResult {
            diff: "--- a/file.txt\n+++ b/file.txt\n@@ -1 +1 @@\n-old\n+new\n".to_string(),
            stats: DiffStats {
                files_changed: 1,
                insertions: 1,
                deletions: 1,
            },
            base_commit: "abc123def456".to_string(),
            head_commit: "789xyz".to_string(),
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"diff\":"));
        assert!(json.contains("\"stats\":"));
        assert!(json.contains("\"base_commit\":\"abc123def456\""));
        assert!(json.contains("\"head_commit\":\"789xyz\""));
    }

    #[test]
    fn repo_diff_error_displays() {
        let err = RepoDiffError {
            message: "test error".to_string(),
        };
        assert_eq!(format!("{err}"), "test error");
    }

    #[test]
    fn repo_diff_args_rejects_missing_url() {
        let json = r#"{"base_commit": "a", "head_commit": "b"}"#;
        let result: Result<RepoDiffArgs, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn repo_diff_args_rejects_missing_base() {
        let json = r#"{"url": "https://x.com/r.git", "head_commit": "b"}"#;
        let result: Result<RepoDiffArgs, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn repo_diff_args_rejects_missing_head() {
        let json = r#"{"url": "https://x.com/r.git", "base_commit": "a"}"#;
        let result: Result<RepoDiffArgs, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn repo_diff_error_from_git2_error() {
        let git2_err = Git2Error::InvalidUrl;
        let diff_err: RepoDiffError = git2_err.into();
        assert!(diff_err.message.contains("invalid"));
    }

    #[test]
    fn handle_repo_diff_with_invalid_url() {
        let args = RepoDiffArgs {
            url: "not-a-url".to_string(),
            base_commit: "a".to_string(),
            head_commit: "b".to_string(),
        };
        let proxy = ProxyConfig::default();
        let result = handle_repo_diff(args, &proxy);
        assert!(result.is_err());
    }

    #[test]
    fn handle_repo_diff_rejects_file_url() {
        let args = RepoDiffArgs {
            url: "file:///etc/passwd".to_string(),
            base_commit: "a".to_string(),
            head_commit: "b".to_string(),
        };
        let proxy = ProxyConfig::default();
        assert!(handle_repo_diff(args, &proxy).is_err());
    }
}
