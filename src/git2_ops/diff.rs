//! Diff generation between commits via git2.
//!
//! This module provides functionality to generate unified diffs between two
//! commits from a remote repository. This is useful for reviewing changes
//! without downloading the entire repository content.
//!
//! # Security
//!
//! - Uses the same credential callbacks as clone/push operations
//! - Temporary bare repo is cleaned up after diff generation
//! - No source files are written to disk (only git objects)
//! - Credentials are never included in diff output

use git2::{DiffFormat, DiffOptions, FetchOptions, Oid, Repository};
use serde::Serialize;
use tempfile::TempDir;
use tracing::{debug, info};

use super::auth::{create_callbacks, sanitize_url_for_logging, validate_url};
use super::error::Git2Error;

/// Statistics about the diff.
#[derive(Debug, Clone, Serialize, Default)]
pub struct DiffStats {
    /// Number of files changed
    pub files_changed: usize,

    /// Number of insertions (lines added)
    pub insertions: usize,

    /// Number of deletions (lines removed)
    pub deletions: usize,
}

/// Result of generating a diff between two commits.
#[derive(Debug, Clone, Serialize)]
pub struct DiffResult {
    /// Unified diff output
    pub diff: String,

    /// Diff statistics
    pub stats: DiffStats,

    /// Base commit SHA (full)
    pub base_commit: String,

    /// Head commit SHA (full)
    pub head_commit: String,
}

/// Generate a diff between two commits from a remote repository.
///
/// This function:
/// 1. Fetches the repository (or enough of it to have both commits)
/// 2. Looks up both commits
/// 3. Generates a unified diff between their trees
///
/// # Arguments
///
/// * `url` - Repository URL (https:// or git@)
/// * `base_commit` - Base commit SHA (or ref like "main~5")
/// * `head_commit` - Head commit SHA (or ref like "main")
///
/// # Returns
///
/// A `DiffResult` containing the unified diff and statistics.
///
/// # Errors
///
/// Returns `Git2Error` if:
/// - URL validation fails
/// - Fetch fails (network, auth)
/// - Either commit cannot be found
/// - Diff generation fails
///
/// # Security
///
/// Credentials are handled via git2 callbacks and never stored or logged.
/// The temporary bare repository is cleaned up after the operation.
pub fn generate_diff(
    url: &str,
    base_commit: &str,
    head_commit: &str,
    proxy_url: Option<&str>,
) -> Result<DiffResult, Git2Error> {
    info!(
        url = %sanitize_url_for_logging(url),
        base = %base_commit,
        head = %head_commit,
        "generating diff"
    );

    // Validate URL
    validate_url(url)?;

    // Create temp directory for bare repo
    let temp_dir = TempDir::new().map_err(Git2Error::TempDirFailed)?;

    debug!(path = %temp_dir.path().display(), "created temp directory");

    // Initialize bare repository
    let repo = Repository::init_bare(temp_dir.path())
        .map_err(|e| Git2Error::InitFailed(format!("failed to init bare repo: {}", e.message())))?;

    // Fetch all refs to ensure we have both commits
    // Using +refs/*:refs/* to get all branches and tags
    {
        let mut remote = repo
            .remote_anonymous(url)
            .map_err(|e| Git2Error::InitFailed(format!("failed to create remote: {e}")))?;

        let callbacks = create_callbacks();
        let mut fetch_opts = FetchOptions::new();
        fetch_opts.remote_callbacks(callbacks);

        let mut proxy_opts = git2::ProxyOptions::new();
        if let Some(url) = proxy_url {
            proxy_opts.url(url);
        } else {
            proxy_opts.auto();
        }
        fetch_opts.proxy_options(proxy_opts);

        debug!("fetching repository");

        // Fetch all refs
        remote
            .fetch(
                &["+refs/heads/*:refs/heads/*", "+refs/tags/*:refs/tags/*"],
                Some(&mut fetch_opts),
                None,
            )
            .map_err(|e| Git2Error::FetchFailed(e.message().to_string()))?;
    }

    debug!("fetch complete, looking up commits");

    // Resolve base commit
    let base_oid = resolve_commit(&repo, base_commit)?;
    let base = repo
        .find_commit(base_oid)
        .map_err(|e| Git2Error::RefNotFound(format!("base commit {base_commit}: {e}")))?;

    // Resolve head commit
    let head_oid = resolve_commit(&repo, head_commit)?;
    let head = repo
        .find_commit(head_oid)
        .map_err(|e| Git2Error::RefNotFound(format!("head commit {head_commit}: {e}")))?;

    debug!(
        base = %base_oid,
        head = %head_oid,
        "commits resolved"
    );

    // Get trees
    let base_tree = base
        .tree()
        .map_err(|e| Git2Error::Git2(format!("failed to get base tree: {e}")))?;
    let head_tree = head
        .tree()
        .map_err(|e| Git2Error::Git2(format!("failed to get head tree: {e}")))?;

    // Configure diff options
    let mut diff_opts = DiffOptions::new();
    diff_opts.context_lines(3); // Standard 3 lines of context

    // Generate diff
    let diff = repo
        .diff_tree_to_tree(Some(&base_tree), Some(&head_tree), Some(&mut diff_opts))
        .map_err(|e| Git2Error::Git2(format!("failed to generate diff: {e}")))?;

    // Get statistics
    let git_stats = diff
        .stats()
        .map_err(|e| Git2Error::Git2(format!("failed to get diff stats: {e}")))?;

    let stats = DiffStats {
        files_changed: git_stats.files_changed(),
        insertions: git_stats.insertions(),
        deletions: git_stats.deletions(),
    };

    // Format as unified diff
    let mut diff_text = String::new();
    diff.print(DiffFormat::Patch, |_delta, _hunk, line| {
        // Get the line content
        if let Ok(content) = std::str::from_utf8(line.content()) {
            // Add origin character for context/add/delete lines
            match line.origin() {
                '+' | '-' | ' ' => {
                    diff_text.push(line.origin());
                    diff_text.push_str(content);
                }
                // File headers don't need origin char
                _ => {
                    diff_text.push_str(content);
                }
            }
        }
        true
    })
    .map_err(|e| Git2Error::Git2(format!("failed to format diff: {e}")))?;

    info!(
        files = stats.files_changed,
        insertions = stats.insertions,
        deletions = stats.deletions,
        "diff generation complete"
    );

    Ok(DiffResult {
        diff: diff_text,
        stats,
        base_commit: base_oid.to_string(),
        head_commit: head_oid.to_string(),
    })
}

/// Resolve a commit reference to an OID.
///
/// Handles:
/// - Full SHA (40 hex chars)
/// - Short SHA (minimum 4 hex chars)
/// - Branch names (looks up refs/heads/...)
/// - Tag names (looks up refs/tags/...)
fn resolve_commit(repo: &Repository, reference: &str) -> Result<Oid, Git2Error> {
    // Try as direct OID first (full or short SHA)
    if let Ok(oid) = Oid::from_str(reference) {
        return Ok(oid);
    }

    // Try revparse (handles branch names, tags, HEAD~N, etc.)
    if let Ok(obj) = repo.revparse_single(reference) {
        if let Some(commit) = obj.as_commit() {
            return Ok(commit.id());
        }
        // If it's not a commit directly, try to peel to commit
        if let Ok(commit) = obj.peel_to_commit() {
            return Ok(commit.id());
        }
    }

    // Try as branch name
    if let Ok(reference_obj) = repo.find_reference(&format!("refs/heads/{reference}")) {
        if let Ok(commit) = reference_obj.peel_to_commit() {
            return Ok(commit.id());
        }
    }

    // Try as tag name
    if let Ok(reference_obj) = repo.find_reference(&format!("refs/tags/{reference}")) {
        if let Ok(commit) = reference_obj.peel_to_commit() {
            return Ok(commit.id());
        }
    }

    Err(Git2Error::RefNotFound(format!(
        "could not resolve '{reference}' to a commit"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_stats_default() {
        let stats = DiffStats::default();
        assert_eq!(stats.files_changed, 0);
        assert_eq!(stats.insertions, 0);
        assert_eq!(stats.deletions, 0);
    }

    #[test]
    fn diff_stats_serializes() {
        let stats = DiffStats {
            files_changed: 5,
            insertions: 100,
            deletions: 50,
        };
        let json = serde_json::to_string(&stats).unwrap();
        assert!(json.contains("\"files_changed\":5"));
        assert!(json.contains("\"insertions\":100"));
        assert!(json.contains("\"deletions\":50"));
    }

    #[test]
    fn diff_result_serializes() {
        let result = DiffResult {
            diff: "--- a/file.txt\n+++ b/file.txt\n".to_string(),
            stats: DiffStats {
                files_changed: 1,
                insertions: 10,
                deletions: 5,
            },
            base_commit: "abc123".to_string(),
            head_commit: "def456".to_string(),
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"base_commit\":\"abc123\""));
        assert!(json.contains("\"head_commit\":\"def456\""));
    }

    #[test]
    fn generate_diff_rejects_invalid_url() {
        let result = generate_diff("/invalid/path", "abc", "def", None);
        assert!(result.is_err());
    }

    #[test]
    fn generate_diff_rejects_file_url() {
        let result = generate_diff("file:///etc/passwd", "abc", "def", None);
        assert!(result.is_err());
    }
}
