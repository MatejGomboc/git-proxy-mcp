//! Incremental sync (pull) operations via git2.
//!
//! This module provides functionality to fetch changes since a known commit,
//! generating a delta that can be used to update an AI's view of the repository
//! without re-downloading everything.
//!
//! # Data Returned
//!
//! - Unified diff showing all changes
//! - Tar.gz of changed/added files at HEAD
//! - List of deleted files
//! - Statistics (commits, files changed, insertions, deletions)
//!
//! # Security
//!
//! - Uses the same credential callbacks as clone/push operations
//! - Temporary bare repo is cleaned up after operation
//! - No source files are written to disk
//! - Credentials are never included in output

use base64::Engine;
use flate2::write::GzEncoder;
use flate2::Compression;
use git2::{Delta, DiffFormat, DiffOptions, FetchOptions, Oid, Repository, TreeWalkMode};
use serde::Serialize;
use tar::{Builder, Header};
use tempfile::TempDir;
use tracing::{debug, info};

use super::auth::{create_callbacks, sanitize_url_for_logging, validate_url};
use super::error::Git2Error;

/// Statistics about the incremental sync.
#[derive(Debug, Clone, Serialize, Default)]
pub struct PullStats {
    /// Number of new commits
    pub commits: usize,

    /// Number of files changed (added + modified + deleted)
    pub files_changed: usize,

    /// Number of files added
    pub files_added: usize,

    /// Number of files modified
    pub files_modified: usize,

    /// Number of files deleted
    pub files_deleted: usize,

    /// Total insertions (lines added)
    pub insertions: usize,

    /// Total deletions (lines removed)
    pub deletions: usize,
}

/// Information about a changed file.
#[derive(Debug, Clone, Serialize)]
pub struct ChangedFile {
    /// File path
    pub path: String,

    /// Change type: "added", "modified", "deleted", "renamed"
    pub change_type: String,

    /// Old path (for renames)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
}

/// Result of an incremental pull operation.
#[derive(Debug, Clone, Serialize)]
pub struct PullResult {
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

/// Fetch changes since a known commit.
///
/// This function:
/// 1. Fetches the repository (or enough of it to have both commits)
/// 2. Generates a unified diff between `since_commit` and HEAD
/// 3. Creates a tar.gz of changed/added files
/// 4. Returns comprehensive delta information
///
/// # Arguments
///
/// * `url` - Repository URL (https:// or git@)
/// * `branch` - Branch name to sync
/// * `since_commit` - The commit SHA that the AI already has
///
/// # Returns
///
/// A `PullResult` containing the diff, changed files, and stats.
///
/// # Errors
///
/// Returns `Git2Error` if:
/// - URL validation fails
/// - Fetch fails (network, auth)
/// - `since_commit` cannot be found
/// - Diff generation fails
///
/// # Security
///
/// Credentials are handled via git2 callbacks and never stored or logged.
/// The temporary bare repository is cleaned up after the operation.
#[allow(clippy::too_many_lines)] // Complex operation with many steps
pub fn pull_changes(url: &str, branch: &str, since_commit: &str) -> Result<PullResult, Git2Error> {
    info!(
        url = %sanitize_url_for_logging(url),
        branch = %branch,
        since = %since_commit,
        "pulling changes"
    );

    // Validate URL
    validate_url(url)?;

    // Create temp directory for bare repo
    let temp_dir = TempDir::new().map_err(Git2Error::TempDirFailed)?;

    debug!(path = %temp_dir.path().display(), "created temp directory");

    // Initialize bare repository
    let repo = Repository::init_bare(temp_dir.path()).map_err(|e| {
        Git2Error::InitFailed(format!("failed to init bare repo: {}", e.message()))
    })?;

    // Fetch the branch
    let refspec = format!("+refs/heads/{branch}:refs/heads/{branch}");
    {
        let mut remote = repo
            .remote_anonymous(url)
            .map_err(|e| Git2Error::InitFailed(format!("failed to create remote: {e}")))?;

        let callbacks = create_callbacks();
        let mut fetch_opts = FetchOptions::new();
        fetch_opts.remote_callbacks(callbacks);

        debug!(refspec = %refspec, "fetching branch");

        remote
            .fetch(&[&refspec], Some(&mut fetch_opts), None)
            .map_err(|e| Git2Error::FetchFailed(e.message().to_string()))?;
    }

    debug!("fetch complete, looking up commits");

    // Resolve since_commit
    let base_oid = Oid::from_str(since_commit)
        .map_err(|_| Git2Error::RefNotFound(format!("invalid commit SHA: {since_commit}")))?;

    // Check if we have the base commit
    let base_commit = repo
        .find_commit(base_oid)
        .map_err(|_| Git2Error::RefNotFound(format!("base commit not found: {since_commit}")))?;

    // Get HEAD of the branch
    let head_ref = repo
        .find_reference(&format!("refs/heads/{branch}"))
        .map_err(|_| Git2Error::RefNotFound(format!("branch not found: {branch}")))?;

    let head_commit = head_ref
        .peel_to_commit()
        .map_err(|e| Git2Error::RefNotFound(format!("failed to get HEAD commit: {e}")))?;

    let head_oid = head_commit.id();

    // Check if already up to date
    if base_oid == head_oid {
        info!("repository is up to date");
        return Ok(PullResult {
            diff: String::new(),
            files_archive: String::new(),
            changed_files: Vec::new(),
            deleted_files: Vec::new(),
            base_commit: base_oid.to_string(),
            new_commit: head_oid.to_string(),
            stats: PullStats::default(),
            up_to_date: true,
        });
    }

    debug!(
        base = %base_oid,
        head = %head_oid,
        "generating diff"
    );

    // Count commits between base and head
    let commit_count = count_commits_between(&repo, base_oid, head_oid)?;

    // Get trees for diff
    let base_tree = base_commit
        .tree()
        .map_err(|e| Git2Error::Git2(format!("failed to get base tree: {e}")))?;
    let head_tree = head_commit
        .tree()
        .map_err(|e| Git2Error::Git2(format!("failed to get head tree: {e}")))?;

    // Configure diff options
    let mut diff_opts = DiffOptions::new();
    diff_opts.context_lines(3);

    // Generate diff
    let diff = repo
        .diff_tree_to_tree(Some(&base_tree), Some(&head_tree), Some(&mut diff_opts))
        .map_err(|e| Git2Error::Git2(format!("failed to generate diff: {e}")))?;

    // Get diff statistics
    let git_stats = diff
        .stats()
        .map_err(|e| Git2Error::Git2(format!("failed to get diff stats: {e}")))?;

    // Collect changed files
    let mut changed_files = Vec::new();
    let mut deleted_files = Vec::new();
    let mut files_to_archive: Vec<String> = Vec::new();
    let mut files_added = 0;
    let mut files_modified = 0;
    let mut files_deleted = 0;

    for delta_idx in 0..diff.deltas().len() {
        if let Some(delta) = diff.get_delta(delta_idx) {
            let status = delta.status();
            let new_file = delta.new_file();
            let old_file = delta.old_file();

            let path = new_file
                .path()
                .or_else(|| old_file.path())
                .and_then(|p| p.to_str())
                .unwrap_or("")
                .to_string();

            if path.is_empty() {
                continue;
            }

            let (change_type, old_path) = match status {
                Delta::Added => {
                    files_added += 1;
                    files_to_archive.push(path.clone());
                    ("added".to_string(), None)
                }
                Delta::Deleted => {
                    files_deleted += 1;
                    deleted_files.push(path.clone());
                    ("deleted".to_string(), None)
                }
                Delta::Modified => {
                    files_modified += 1;
                    files_to_archive.push(path.clone());
                    ("modified".to_string(), None)
                }
                Delta::Renamed => {
                    files_modified += 1;
                    files_to_archive.push(path.clone());
                    let old = old_file.path().and_then(|p| p.to_str()).map(String::from);
                    ("renamed".to_string(), old)
                }
                Delta::Copied => {
                    files_added += 1;
                    files_to_archive.push(path.clone());
                    ("copied".to_string(), None)
                }
                _ => continue, // Skip unmodified, ignored, etc.
            };

            changed_files.push(ChangedFile {
                path,
                change_type,
                old_path,
            });
        }
    }

    // Format diff as unified text
    let mut diff_text = String::new();
    diff.print(DiffFormat::Patch, |_delta, _hunk, line| {
        if let Ok(content) = std::str::from_utf8(line.content()) {
            match line.origin() {
                '+' | '-' | ' ' => {
                    diff_text.push(line.origin());
                    diff_text.push_str(content);
                }
                _ => {
                    diff_text.push_str(content);
                }
            }
        }
        true
    })
    .map_err(|e| Git2Error::Git2(format!("failed to format diff: {e}")))?;

    // Create tar.gz of changed/added files
    let files_archive = if files_to_archive.is_empty() {
        String::new()
    } else {
        create_files_archive(&repo, &head_tree, &files_to_archive)?
    };

    let stats = PullStats {
        commits: commit_count,
        files_changed: git_stats.files_changed(),
        files_added,
        files_modified,
        files_deleted,
        insertions: git_stats.insertions(),
        deletions: git_stats.deletions(),
    };

    info!(
        commits = stats.commits,
        files = stats.files_changed,
        added = stats.files_added,
        modified = stats.files_modified,
        deleted = stats.files_deleted,
        insertions = stats.insertions,
        deletions = stats.deletions,
        "pull complete"
    );

    Ok(PullResult {
        diff: diff_text,
        files_archive,
        changed_files,
        deleted_files,
        base_commit: base_oid.to_string(),
        new_commit: head_oid.to_string(),
        stats,
        up_to_date: false,
    })
}

/// Count commits between two commits (base exclusive, head inclusive).
fn count_commits_between(repo: &Repository, base: Oid, head: Oid) -> Result<usize, Git2Error> {
    let mut revwalk = repo
        .revwalk()
        .map_err(|e| Git2Error::Git2(format!("failed to create revwalk: {e}")))?;

    revwalk
        .push(head)
        .map_err(|e| Git2Error::Git2(format!("failed to push head: {e}")))?;

    revwalk
        .hide(base)
        .map_err(|e| Git2Error::Git2(format!("failed to hide base: {e}")))?;

    let count = revwalk.count();
    Ok(count)
}

/// Create a tar.gz archive of specified files from a tree.
fn create_files_archive(
    repo: &Repository,
    tree: &git2::Tree,
    files: &[String],
) -> Result<String, Git2Error> {
    let mut buffer = Vec::new();
    {
        let encoder = GzEncoder::new(&mut buffer, Compression::fast());
        let mut tar = Builder::new(encoder);

        // Walk the tree and add matching files
        tree.walk(TreeWalkMode::PreOrder, |dir, entry| {
            if entry.kind() != Some(git2::ObjectType::Blob) {
                return git2::TreeWalkResult::Ok;
            }

            let path = if dir.is_empty() {
                entry.name().unwrap_or("").to_string()
            } else {
                format!("{}{}", dir, entry.name().unwrap_or(""))
            };

            // Check if this file is in our list
            if !files.contains(&path) {
                return git2::TreeWalkResult::Ok;
            }

            // Get blob content
            if let Ok(blob) = repo.find_blob(entry.id()) {
                let content = blob.content();

                let mut header = Header::new_gnu();
                #[allow(clippy::cast_possible_truncation)]
                header.set_size(content.len() as u64);
                header.set_mode(0o644);
                header.set_mtime(0);
                header.set_cksum();

                if let Err(e) = tar.append_data(&mut header, &path, content) {
                    debug!(path = %path, error = %e, "failed to add file to archive");
                }
            }

            git2::TreeWalkResult::Ok
        })
        .map_err(|e| Git2Error::Git2(format!("failed to walk tree: {e}")))?;

        tar.finish()
            .map_err(|e| Git2Error::Git2(format!("failed to finish tar: {e}")))?;
    }

    // Base64 encode the archive
    let encoded = base64::engine::general_purpose::STANDARD.encode(&buffer);
    Ok(encoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pull_stats_default() {
        let stats = PullStats::default();
        assert_eq!(stats.commits, 0);
        assert_eq!(stats.files_changed, 0);
        assert_eq!(stats.insertions, 0);
    }

    #[test]
    fn pull_stats_serializes() {
        let stats = PullStats {
            commits: 5,
            files_changed: 10,
            files_added: 3,
            files_modified: 5,
            files_deleted: 2,
            insertions: 100,
            deletions: 50,
        };
        let json = serde_json::to_string(&stats).unwrap();
        assert!(json.contains("\"commits\":5"));
        assert!(json.contains("\"files_added\":3"));
    }

    #[test]
    fn changed_file_serializes() {
        let cf = ChangedFile {
            path: "src/main.rs".to_string(),
            change_type: "modified".to_string(),
            old_path: None,
        };
        let json = serde_json::to_string(&cf).unwrap();
        assert!(json.contains("\"path\":\"src/main.rs\""));
        assert!(json.contains("\"change_type\":\"modified\""));
        assert!(!json.contains("old_path")); // skipped when None
    }

    #[test]
    fn changed_file_with_rename_serializes() {
        let cf = ChangedFile {
            path: "src/new.rs".to_string(),
            change_type: "renamed".to_string(),
            old_path: Some("src/old.rs".to_string()),
        };
        let json = serde_json::to_string(&cf).unwrap();
        assert!(json.contains("\"old_path\":\"src/old.rs\""));
    }

    #[test]
    fn pull_result_serializes() {
        let result = PullResult {
            diff: "--- a/file.txt\n+++ b/file.txt\n".to_string(),
            files_archive: "base64data".to_string(),
            changed_files: vec![],
            deleted_files: vec!["old.txt".to_string()],
            base_commit: "abc123".to_string(),
            new_commit: "def456".to_string(),
            stats: PullStats::default(),
            up_to_date: false,
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"base_commit\":\"abc123\""));
        assert!(json.contains("\"up_to_date\":false"));
    }

    #[test]
    fn pull_changes_rejects_invalid_url() {
        let result = pull_changes("/invalid/path", "main", "abc123");
        assert!(result.is_err());
    }

    #[test]
    fn pull_changes_rejects_file_url() {
        let result = pull_changes("file:///etc/passwd", "main", "abc123");
        assert!(result.is_err());
    }
}
