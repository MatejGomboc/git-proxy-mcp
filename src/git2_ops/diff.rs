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
use super::error::{sanitize_error_message, Git2Error};

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

    generate_diff_inner(url, base_commit, head_commit, proxy_url)
}

/// Fetches the repo and diffs `base_commit`..`head_commit` — the body of
/// [`generate_diff`] after URL validation. Split out so tests can drive the
/// fetch/resolve/diff path against a local `file://` remote; [`generate_diff`]
/// still rejects `file://` via [`validate_url`] before delegating here.
fn generate_diff_inner(
    url: &str,
    base_commit: &str,
    head_commit: &str,
    proxy_url: Option<&str>,
) -> Result<DiffResult, Git2Error> {
    // Create temp directory for bare repo
    let temp_dir = TempDir::new().map_err(Git2Error::TempDirFailed)?;

    debug!(path = %temp_dir.path().display(), "created temp directory");

    // Initialise bare repository
    let repo = Repository::init_bare(temp_dir.path())
        .map_err(|e| Git2Error::InitFailed(format!("failed to init bare repo: {}", e.message())))?;

    // Fetch all refs to ensure we have both commits
    // Using +refs/*:refs/* to get all branches and tags
    {
        let mut remote = repo.remote_anonymous(url).map_err(|e| {
            Git2Error::InitFailed(format!(
                "failed to create remote: {}",
                sanitize_error_message(e.message())
            ))
        })?;

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
            .map_err(|e| Git2Error::from_fetch(&e))?;
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
    // Try as a direct OID, but only for a full-length (40-char) SHA. A shorter
    // hex string must NOT take this path: `Oid::from_str` zero-pads the missing
    // nibbles into a bogus OID (e.g. "abc1" -> abc1000…0) rather than resolving
    // the abbreviation, so it would never reach the `revparse_single` below
    // (which does resolve short SHAs against the repo). A full SHA is parsed
    // directly because the object may not be present yet — the caller verifies
    // existence via `find_commit`.
    if reference.len() == 40 {
        if let Ok(oid) = Oid::from_str(reference) {
            return Ok(oid);
        }
    }

    // Try revparse (handles full/abbreviated SHAs, branch names, tags, HEAD~N)
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

    #[test]
    fn generate_diff_fails_fast_on_unreachable_host() {
        // Reaches the fetch (past validate_url) against a host that RSTs
        // immediately, exercising the credential-safe fetch error mapping.
        let result = generate_diff("https://127.0.0.1:1/o/r.git", "abc", "def", None);
        assert!(matches!(result, Err(Git2Error::FetchFailed(_))));
    }

    /// Helper: build a test bare repo with two commits, return temp dir + commit OIDs.
    fn build_test_repo_with_two_commits() -> (tempfile::TempDir, git2::Oid, git2::Oid) {
        let temp = tempfile::TempDir::new().unwrap();
        let (oid1, oid2) = {
            let repo = Repository::init_bare(temp.path()).unwrap();
            let signature = git2::Signature::now("Test", "test@example.com").unwrap();

            // Commit 1: README.md only
            let blob1 = repo.blob(b"# Test\n").unwrap();
            let mut tb = repo.treebuilder(None).unwrap();
            tb.insert("README.md", blob1, 0o100_644).unwrap();
            let tree1_oid = tb.write().unwrap();
            let tree1 = repo.find_tree(tree1_oid).unwrap();
            let commit1 = repo
                .commit(Some("HEAD"), &signature, &signature, "first", &tree1, &[])
                .unwrap();

            // Commit 2: README.md modified + main.rs added
            let blob2 = repo.blob(b"# Test (updated)\n").unwrap();
            let blob3 = repo.blob(b"fn main() {}\n").unwrap();
            let mut tb = repo.treebuilder(None).unwrap();
            tb.insert("README.md", blob2, 0o100_644).unwrap();
            tb.insert("main.rs", blob3, 0o100_644).unwrap();
            let tree2_oid = tb.write().unwrap();
            let tree2 = repo.find_tree(tree2_oid).unwrap();
            let parent = repo.find_commit(commit1).unwrap();
            let commit2 = repo
                .commit(
                    Some("HEAD"),
                    &signature,
                    &signature,
                    "second",
                    &tree2,
                    &[&parent],
                )
                .unwrap();

            // Tag the second commit (lightweight: a direct ref to the commit).
            repo.tag_lightweight(
                "v1.0",
                &repo.find_commit(commit2).unwrap().into_object(),
                false,
            )
            .unwrap();

            // Annotated tag (a tag *object*, not a direct ref) so
            // `resolve_commit`'s peel-to-commit path can be exercised.
            repo.tag(
                "v2.0",
                &repo.find_commit(commit2).unwrap().into_object(),
                &signature,
                "release 2.0",
                false,
            )
            .unwrap();

            (commit1, commit2)
        };
        (temp, oid1, oid2)
    }

    #[test]
    fn resolve_commit_full_sha() {
        let (temp, oid1, _) = build_test_repo_with_two_commits();
        let repo = Repository::open_bare(temp.path()).unwrap();
        let resolved = resolve_commit(&repo, &oid1.to_string()).unwrap();
        assert_eq!(resolved, oid1);
    }

    #[test]
    fn resolve_commit_branch_name() {
        let (temp, _, oid2) = build_test_repo_with_two_commits();
        let repo = Repository::open_bare(temp.path()).unwrap();
        // The default branch is "master" or "main" depending on git config.
        // After commit, HEAD points to the active branch.
        let head = repo.head().unwrap();
        let branch_name = head.shorthand().unwrap().to_string();
        let resolved = resolve_commit(&repo, &branch_name).unwrap();
        assert_eq!(resolved, oid2);
    }

    #[test]
    fn resolve_commit_tag_name() {
        let (temp, _, oid2) = build_test_repo_with_two_commits();
        let repo = Repository::open_bare(temp.path()).unwrap();
        let resolved = resolve_commit(&repo, "v1.0").unwrap();
        assert_eq!(resolved, oid2);
    }

    #[test]
    fn resolve_commit_annotated_tag() {
        // An annotated tag resolves via `revparse_single` to a Tag *object*,
        // which `resolve_commit` then peels to the underlying commit.
        let (temp, _, oid2) = build_test_repo_with_two_commits();
        let repo = Repository::open_bare(temp.path()).unwrap();
        let resolved = resolve_commit(&repo, "v2.0").unwrap();
        assert_eq!(resolved, oid2);
    }

    #[test]
    fn resolve_commit_invalid_reference() {
        let (temp, _, _) = build_test_repo_with_two_commits();
        let repo = Repository::open_bare(temp.path()).unwrap();
        let result = resolve_commit(&repo, "nonexistent_ref");
        assert!(result.is_err());
    }

    #[test]
    fn resolve_commit_head_relative() {
        let (temp, _, oid2) = build_test_repo_with_two_commits();
        let repo = Repository::open_bare(temp.path()).unwrap();
        // HEAD~0 = HEAD = second commit
        let resolved = resolve_commit(&repo, "HEAD").unwrap();
        assert_eq!(resolved, oid2);
    }

    #[test]
    fn resolve_commit_invalid_sha_format() {
        let (temp, _, _) = build_test_repo_with_two_commits();
        let repo = Repository::open_bare(temp.path()).unwrap();
        let result = resolve_commit(&repo, "not-a-sha-or-anything");
        assert!(result.is_err());
    }

    #[test]
    fn resolve_commit_short_sha() {
        let (temp, _, oid2) = build_test_repo_with_two_commits();
        let repo = Repository::open_bare(temp.path()).unwrap();
        let short = &oid2.to_string()[..10];
        let resolved = resolve_commit(&repo, short).unwrap();
        assert_eq!(
            resolved, oid2,
            "short SHA should resolve to the full commit"
        );
    }

    /// Builds a `file://` URL for a local path (Windows-friendly).
    fn local_file_url(path: &std::path::Path) -> String {
        let raw = path.display().to_string();
        if cfg!(windows) {
            format!("file:///{}", raw.replace('\\', "/"))
        } else {
            format!("file://{raw}")
        }
    }

    #[test]
    fn generate_diff_inner_diffs_two_commits() {
        // Drives the whole fetch/resolve/diff/format body against a local
        // remote (bypassing validate_url's file:// rejection via the inner fn).
        let (source, oid1, oid2) = build_test_repo_with_two_commits();
        let result = generate_diff_inner(
            &local_file_url(source.path()),
            &oid1.to_string(),
            &oid2.to_string(),
            None,
        )
        .unwrap();

        assert_eq!(result.base_commit, oid1.to_string());
        assert_eq!(result.head_commit, oid2.to_string());
        // commit1 -> commit2: README.md modified + main.rs added.
        assert_eq!(result.stats.files_changed, 2);
        assert!(result.diff.contains("main.rs"), "diff: {}", result.diff);
        assert!(result.diff.contains("# Test (updated)"));
    }

    #[test]
    fn generate_diff_inner_resolves_tag_ref() {
        // head given as a tag name exercises resolve_commit's revparse/tag path
        // inside the fetch/diff flow.
        let (source, oid1, oid2) = build_test_repo_with_two_commits();
        let result = generate_diff_inner(
            &local_file_url(source.path()),
            &oid1.to_string(),
            "v1.0",
            None,
        )
        .unwrap();
        assert_eq!(result.head_commit, oid2.to_string());
    }

    #[test]
    fn generate_diff_inner_errors_on_missing_commit() {
        // A well-formed but absent SHA resolves (Oid::from_str) yet isn't in the
        // fetched repo, so find_commit fails with RefNotFound.
        let (source, _, oid2) = build_test_repo_with_two_commits();
        let result = generate_diff_inner(
            &local_file_url(source.path()),
            "0000000000000000000000000000000000000000",
            &oid2.to_string(),
            None,
        );
        assert!(matches!(result, Err(Git2Error::RefNotFound(_))));
    }

    #[test]
    fn generate_diff_inner_errors_on_nonexistent_local_remote() {
        let tmp = tempfile::TempDir::new().unwrap();
        let missing = tmp.path().join("no-such-repo");
        let result = generate_diff_inner(&local_file_url(&missing), "abc", "def", None);
        assert!(result.is_err());
    }
}
