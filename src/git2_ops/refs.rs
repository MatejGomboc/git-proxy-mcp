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
pub fn list_remote_refs(url: &str) -> Result<RefsResult, Git2Error> {
    info!(url = %sanitize_url_for_logging(url), "listing remote refs");

    // Validate URL
    validate_url(url)?;

    // Create a temporary repository for the remote connection (auto-cleaned on drop)
    let temp_dir = TempDir::new().map_err(Git2Error::TempDirFailed)?;
    let repo = Repository::init_bare(temp_dir.path())?;

    let (mut branches, mut tags, default_branch, head_oid) = {
        // Create remote with callbacks in a scope so it gets dropped before repo
        let mut remote = repo.remote_anonymous(url)?;
        let callbacks = create_callbacks();

        // Connect to remote and get refs
        debug!("connecting to remote");
        remote.connect_auth(Direction::Fetch, Some(callbacks), None)?;

        let remote_refs = remote.list()?;

        let mut branches = Vec::new();
        let mut tags = Vec::new();
        let mut head_oid: Option<String> = None;

        for head in remote_refs {
            let name = head.name();
            let oid = head.oid();

            // Capture HEAD's OID for default branch detection
            if name == "HEAD" {
                head_oid = Some(oid.to_string());
                continue;
            }

            // Parse the reference type
            if let Some(branch_name) = name.strip_prefix("refs/heads/") {
                branches.push(RefInfo {
                    name: name.to_string(),
                    short_name: branch_name.to_string(),
                    commit: oid.to_string(),
                });
            } else if let Some(tag_name) = name.strip_prefix("refs/tags/") {
                // Skip ^{} peeled refs (they're duplicates for annotated tags)
                if !tag_name.ends_with("^{}") {
                    tags.push(RefInfo {
                        name: name.to_string(),
                        short_name: tag_name.to_string(),
                        commit: oid.to_string(),
                    });
                }
            }
        }

        // Determine default branch from HEAD OID
        #[allow(clippy::option_if_let_else)]
        let default_branch = if let Some(ref head_commit) = head_oid {
            branches
                .iter()
                .find(|b| &b.commit == head_commit)
                .map_or_else(|| "main".to_string(), |b| b.short_name.clone())
        } else {
            "main".to_string()
        };

        (branches, tags, default_branch, head_oid)
    };
    // remote is now dropped, repo can be dropped

    // Sort branches and tags alphabetically
    branches.sort_by(|a, b| a.short_name.cmp(&b.short_name));
    tags.sort_by(|a, b| a.short_name.cmp(&b.short_name));

    let total_refs = branches.len() + tags.len();

    // Clean up: drop repo first, then temp_dir cleans itself
    drop(repo);
    drop(temp_dir);

    info!(
        branches = branches.len(),
        tags = tags.len(),
        default_branch = %default_branch,
        head_oid = ?head_oid,
        "refs listing complete"
    );

    Ok(RefsResult {
        branches,
        tags,
        default_branch,
        total_refs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let result = list_remote_refs("/invalid/path");
        assert!(result.is_err());
    }

    #[test]
    fn list_remote_refs_rejects_file_url() {
        let result = list_remote_refs("file:///etc/passwd");
        assert!(result.is_err());
    }
}
