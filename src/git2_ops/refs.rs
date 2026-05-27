//! Remote reference listing via git2.
//!
//! This module provides functionality to list branches and tags from a remote
//! repository without cloning it. This is equivalent to `git ls-remote`.
//!
//! # Security
//!
//! - Uses the same credential callbacks as clone/push operations
//! - No repository data is downloaded (only ref names and OIDs)
//! - No files are written to disk

use git2::{Direction, Repository};
use serde::Serialize;
use tempfile::TempDir;
use tracing::{debug, info};

use super::auth::{create_callbacks, sanitize_url_for_logging, validate_url};
use super::error::Git2Error;

/// Information about a single Git reference.
#[derive(Debug, Clone, Serialize)]
pub struct RefInfo {
    /// Full reference name (e.g., "refs/heads/main", "refs/tags/v1.0.0")
    pub name: String,

    /// Short reference name (e.g., "main", "v1.0.0")
    pub short_name: String,

    /// Commit SHA that this reference points to
    pub commit: String,
}

/// Result of listing remote references.
#[derive(Debug, Clone, Serialize)]
pub struct RefsResult {
    /// List of branches
    pub branches: Vec<RefInfo>,

    /// List of tags
    pub tags: Vec<RefInfo>,

    /// Default branch name (e.g., "main")
    pub default_branch: String,

    /// Total number of references found
    pub total_refs: usize,
}

/// List branches and tags from a remote repository.
///
/// This function connects to the remote and retrieves the list of references
/// without downloading any repository data. It's equivalent to `git ls-remote`.
///
/// # Arguments
///
/// * `url` - Repository URL (https:// or git@)
///
/// # Returns
///
/// A `RefsResult` containing branches, tags, and the default branch.
///
/// # Errors
///
/// Returns `Git2Error` if:
/// - URL validation fails
/// - Connection to remote fails
/// - Authentication fails
///
/// # Security
///
/// Credentials are handled via git2 callbacks and never stored or logged.
pub fn list_remote_refs(url: &str, proxy_url: Option<&str>) -> Result<RefsResult, Git2Error> {
    info!(url = %sanitize_url_for_logging(url), "listing remote refs");

    // Validate URL (rejects file://, ext::, and other non-network schemes).
    validate_url(url)?;

    list_refs_inner(url, proxy_url)
}

/// Connects to `url` and lists its refs — the body of [`list_remote_refs`].
///
/// Split out so tests can exercise the connect/list/parse path against a local
/// `file://` remote: [`list_remote_refs`] rejects non-network URLs via
/// [`validate_url`] before reaching here, but this helper does not, so a test
/// can point it at a local bare repository without weakening that guard.
fn list_refs_inner(url: &str, proxy_url: Option<&str>) -> Result<RefsResult, Git2Error> {
    // Temporary repository for the remote connection (auto-cleaned on drop).
    let temp_dir = TempDir::new().map_err(Git2Error::TempDirFailed)?;
    let repo = Repository::init_bare(temp_dir.path())?;

    let (mut branches, mut tags, default_branch) = {
        // Create remote with callbacks in a scope so it gets dropped before repo.
        let mut remote = repo.remote_anonymous(url)?;
        let callbacks = create_callbacks();

        let mut proxy_opts = git2::ProxyOptions::new();
        if let Some(proxy) = proxy_url {
            proxy_opts.url(proxy);
        } else {
            proxy_opts.auto();
        }

        debug!("connecting to remote");
        remote.connect_auth(Direction::Fetch, Some(callbacks), Some(proxy_opts))?;

        // The remote's advertised HEAD symref (e.g. b"refs/heads/main") is the
        // authoritative default branch. Matching HEAD's OID against the branch
        // list instead is ambiguous when several branches point at the same
        // commit (e.g. a freshly-created branch off the default).
        let default_branch = resolve_default_branch(remote.default_branch().ok().as_deref());

        let remote_refs = remote.list()?;

        let mut branches = Vec::new();
        let mut tags = Vec::new();

        for head in remote_refs {
            let name = head.name();
            let oid = head.oid();

            // HEAD is resolved via the symref above, not listed as a branch.
            if name == "HEAD" {
                continue;
            }

            // Parse the reference type.
            if let Some(branch_name) = name.strip_prefix("refs/heads/") {
                branches.push(RefInfo {
                    name: name.to_string(),
                    short_name: branch_name.to_string(),
                    commit: oid.to_string(),
                });
            } else if let Some(tag_name) = name.strip_prefix("refs/tags/") {
                // Skip ^{} peeled refs (duplicates emitted for annotated tags).
                if !tag_name.ends_with("^{}") {
                    tags.push(RefInfo {
                        name: name.to_string(),
                        short_name: tag_name.to_string(),
                        commit: oid.to_string(),
                    });
                }
            }
        }

        (branches, tags, default_branch)
    };
    // remote is now dropped, repo can be dropped

    // Sort branches and tags alphabetically by short name.
    branches.sort_by(|a, b| a.short_name.cmp(&b.short_name));
    tags.sort_by(|a, b| a.short_name.cmp(&b.short_name));

    let total_refs = branches.len() + tags.len();

    // Clean up: drop repo first, then temp_dir cleans itself.
    drop(repo);
    drop(temp_dir);

    info!(branches = branches.len(), tags = tags.len(), default_branch = %default_branch, "refs listing complete");

    Ok(RefsResult {
        branches,
        tags,
        default_branch,
        total_refs,
    })
}

/// Resolves the default branch name from the remote's advertised `HEAD` symref.
///
/// `head_symref` is the raw bytes returned by [`git2::Remote::default_branch`]
/// (e.g. `b"refs/heads/main"`). Falls back to `"main"` when the symref is
/// absent, non-UTF-8, not under `refs/heads/`, or empty.
fn resolve_default_branch(head_symref: Option<&[u8]>) -> String {
    head_symref
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
        .map(str::trim)
        .and_then(|name| name.strip_prefix("refs/heads/"))
        .filter(|name| !name.is_empty())
        .map_or_else(|| "main".to_string(), ToString::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a `file://` URL for a local repository path, translating Windows
    /// backslashes (mirrors the helper in `clone.rs`'s tests).
    fn local_file_url(path: &std::path::Path) -> String {
        let raw = path.display().to_string();
        if cfg!(windows) {
            format!("file:///{}", raw.replace('\\', "/"))
        } else {
            format!("file://{raw}")
        }
    }

    #[test]
    fn ref_info_serializes() {
        let info = RefInfo {
            name: "refs/heads/main".to_string(),
            short_name: "main".to_string(),
            commit: "abc123".to_string(),
        };
        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("\"name\":\"refs/heads/main\""));
        assert!(json.contains("\"short_name\":\"main\""));
        assert!(json.contains("\"commit\":\"abc123\""));
    }

    #[test]
    fn refs_result_serializes() {
        let result = RefsResult {
            branches: vec![RefInfo {
                name: "refs/heads/main".to_string(),
                short_name: "main".to_string(),
                commit: "abc123".to_string(),
            }],
            tags: vec![RefInfo {
                name: "refs/tags/v1.0.0".to_string(),
                short_name: "v1.0.0".to_string(),
                commit: "def456".to_string(),
            }],
            default_branch: "main".to_string(),
            total_refs: 2,
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"default_branch\":\"main\""));
        assert!(json.contains("\"total_refs\":2"));
    }

    #[test]
    fn list_remote_refs_rejects_invalid_url() {
        let result = list_remote_refs("/invalid/path", None);
        assert!(result.is_err());
    }

    #[test]
    fn list_remote_refs_rejects_file_url() {
        let result = list_remote_refs("file:///etc/passwd", None);
        assert!(result.is_err());
    }

    #[test]
    fn resolve_default_branch_uses_head_symref() {
        assert_eq!(
            resolve_default_branch(Some(b"refs/heads/develop")),
            "develop"
        );
    }

    #[test]
    fn resolve_default_branch_keeps_nested_branch_name() {
        assert_eq!(
            resolve_default_branch(Some(b"refs/heads/feature/login")),
            "feature/login"
        );
    }

    #[test]
    fn resolve_default_branch_trims_trailing_noise() {
        assert_eq!(resolve_default_branch(Some(b"refs/heads/main\n")), "main");
    }

    #[test]
    fn resolve_default_branch_falls_back_when_absent() {
        assert_eq!(resolve_default_branch(None), "main");
    }

    #[test]
    fn resolve_default_branch_falls_back_on_non_heads_ref() {
        assert_eq!(resolve_default_branch(Some(b"refs/tags/v1.0")), "main");
    }

    #[test]
    fn resolve_default_branch_falls_back_on_invalid_utf8() {
        assert_eq!(resolve_default_branch(Some(&[0xff, 0xfe, 0x00])), "main");
    }

    #[test]
    fn resolve_default_branch_falls_back_on_empty_branch_name() {
        assert_eq!(resolve_default_branch(Some(b"refs/heads/")), "main");
    }

    #[test]
    fn list_refs_inner_reads_branches_tags_and_default_branch() {
        // Build a bare "remote" with HEAD -> main, a develop branch pointing at
        // the same commit as main, a lightweight tag and an annotated tag; then
        // list its refs over file:// (mirroring clone.rs's local-remote test).
        // Calling the inner helper directly bypasses validate_url's file://
        // rejection without weakening it.
        let source = TempDir::new().unwrap();
        let repo = Repository::init_bare(source.path()).unwrap();
        repo.set_head("refs/heads/main").unwrap();

        let sig = git2::Signature::now("Test", "test@example.com").unwrap();
        let tree_oid = repo.treebuilder(None).unwrap().write().unwrap();
        let tree = repo.find_tree(tree_oid).unwrap();
        let commit_oid = repo
            .commit(Some("refs/heads/main"), &sig, &sig, "initial", &tree, &[])
            .unwrap();

        // A second branch at the SAME commit as main. The old OID-matching
        // default-branch logic could pick this one (it sorts before "main");
        // the symref-based logic must still resolve to "main".
        repo.reference("refs/heads/develop", commit_oid, true, "create develop")
            .unwrap();

        let commit_obj = repo.find_object(commit_oid, None).unwrap();
        repo.tag_lightweight("v1.0", &commit_obj, false).unwrap();
        repo.tag("v2.0", &commit_obj, &sig, "release 2.0", false)
            .unwrap();

        let result = list_refs_inner(&local_file_url(source.path()), None).unwrap();

        // Branches sorted by short name, HEAD excluded.
        let branch_names: Vec<&str> = result
            .branches
            .iter()
            .map(|b| b.short_name.as_str())
            .collect();
        assert_eq!(branch_names, ["develop", "main"]);

        // Lightweight + annotated tags, sorted, with ^{} peeled entries skipped.
        let tag_names: Vec<&str> = result.tags.iter().map(|t| t.short_name.as_str()).collect();
        assert_eq!(tag_names, ["v1.0", "v2.0"]);

        // Default branch comes from HEAD's symref, not OID matching.
        assert_eq!(result.default_branch, "main");
        assert_eq!(result.total_refs, 4);

        // Full ref names preserved; the lightweight tag points straight at the
        // commit.
        let main = result
            .branches
            .iter()
            .find(|b| b.short_name == "main")
            .unwrap();
        assert_eq!(main.name, "refs/heads/main");
        assert_eq!(main.commit, commit_oid.to_string());
        let v1 = result.tags.iter().find(|t| t.short_name == "v1.0").unwrap();
        assert_eq!(v1.commit, commit_oid.to_string());
    }

    #[test]
    fn list_refs_inner_handles_remote_with_no_refs() {
        let source = TempDir::new().unwrap();
        Repository::init_bare(source.path()).unwrap();

        let result = list_refs_inner(&local_file_url(source.path()), None).unwrap();
        assert!(result.branches.is_empty());
        assert!(result.tags.is_empty());
        assert_eq!(result.total_refs, 0);
        // Nothing advertised -> "main" fallback.
        assert_eq!(result.default_branch, "main");
    }

    #[test]
    fn list_refs_inner_errors_on_nonexistent_local_remote() {
        let tmp = TempDir::new().unwrap();
        let missing = tmp.path().join("does-not-exist");
        let result = list_refs_inner(&local_file_url(&missing), None);
        assert!(result.is_err());
    }

    #[test]
    fn list_refs_inner_accepts_proxy_url() {
        // Exercises the proxy-configuration branch. For local file:// transport
        // the proxy is irrelevant (it only applies to HTTP), so listing still
        // succeeds — we just need the `Some(proxy)` arm to run.
        let source = TempDir::new().unwrap();
        let repo = Repository::init_bare(source.path()).unwrap();
        repo.set_head("refs/heads/main").unwrap();
        let sig = git2::Signature::now("Test", "test@example.com").unwrap();
        let tree_oid = repo.treebuilder(None).unwrap().write().unwrap();
        let tree = repo.find_tree(tree_oid).unwrap();
        repo.commit(Some("refs/heads/main"), &sig, &sig, "initial", &tree, &[])
            .unwrap();

        let result =
            list_refs_inner(&local_file_url(source.path()), Some("http://127.0.0.1:9")).unwrap();
        assert_eq!(result.default_branch, "main");
        assert_eq!(result.branches.len(), 1);
    }
}
