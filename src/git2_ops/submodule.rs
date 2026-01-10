//! Git submodule support for bare repositories.
//!
//! This module provides functionality to detect and fetch submodule contents
//! from bare repositories without a working directory.
//!
//! # How Submodules Work in Git
//!
//! Submodules are tracked in two places:
//! 1. `.gitmodules` file - maps submodule names to URLs and paths
//! 2. Tree entries with mode `160000` - point to specific commits in submodule repos
//!
//! # Security
//!
//! - Each submodule is fetched using the same credential callbacks
//! - Submodule URLs are validated before fetching
//! - No source files are written to disk (bare repos only)

use git2::{ObjectType, Oid, Repository, TreeWalkMode, TreeWalkResult};
use std::collections::HashMap;
use tracing::{debug, trace, warn};

use super::auth::{sanitize_url_for_logging, validate_url};
use super::clone::{fetch_bare, FetchOptions2, FetchResult};
use super::error::Git2Error;

/// Git tree mode for submodule entries (commit references).
const SUBMODULE_MODE: i32 = 0o160_000;

/// Information about a submodule parsed from `.gitmodules`.
#[derive(Debug, Clone)]
pub struct SubmoduleInfo {
    /// Submodule name (section name in .gitmodules)
    pub name: String,
    /// Path where the submodule is located in the tree
    pub path: String,
    /// URL to fetch the submodule from
    pub url: String,
    /// Branch to track (optional, defaults to default branch)
    pub branch: Option<String>,
}

/// A submodule entry found in the tree.
#[derive(Debug, Clone)]
pub struct SubmoduleEntry {
    /// Path in the tree where this submodule is located
    pub path: String,
    /// Commit SHA that the parent repo expects
    pub commit: Oid,
    /// URL to fetch from (from .gitmodules)
    pub url: String,
}

/// Result of fetching a submodule.
pub struct FetchedSubmodule {
    /// The submodule entry information
    pub entry: SubmoduleEntry,
    /// The fetched repository (bare)
    pub fetch_result: FetchResult,
}

/// Parse the `.gitmodules` file content into submodule info.
///
/// The format is INI-like:
/// ```text
/// [submodule "name"]
///     path = some/path
///     url = https://github.com/owner/repo
///     branch = main
/// ```
///
/// # Arguments
///
/// * `content` - Raw content of the `.gitmodules` file
///
/// # Returns
///
/// A map from submodule path to `SubmoduleInfo`.
#[must_use]
pub fn parse_gitmodules(content: &[u8]) -> HashMap<String, SubmoduleInfo> {
    let mut result = HashMap::new();

    let Ok(text) = std::str::from_utf8(content) else {
        return result;
    };

    let mut current_name: Option<String> = None;
    let mut current_path: Option<String> = None;
    let mut current_url: Option<String> = None;
    let mut current_branch: Option<String> = None;

    for line in text.lines() {
        let line = line.trim();

        // Skip empty lines and comments
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }

        // Section header: [submodule "name"]
        if line.starts_with('[') && line.ends_with(']') {
            // Save previous submodule if complete
            if let (Some(name), Some(path), Some(url)) =
                (current_name.take(), current_path.take(), current_url.take())
            {
                result.insert(
                    path.clone(),
                    SubmoduleInfo {
                        name,
                        path,
                        url,
                        branch: current_branch.take(),
                    },
                );
            }

            // Parse new section name - use strip methods for safe slicing
            let inner = line
                .strip_prefix('[')
                .and_then(|s| s.strip_suffix(']'))
                .unwrap_or("");
            if let Some(name) = inner.strip_prefix("submodule \"") {
                if let Some(name) = name.strip_suffix('"') {
                    current_name = Some(name.to_string());
                }
            }
            continue;
        }

        // Key-value pairs
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim();
            let value = value.trim();

            match key {
                "path" => current_path = Some(value.to_string()),
                "url" => current_url = Some(value.to_string()),
                "branch" => current_branch = Some(value.to_string()),
                _ => {} // Ignore unknown keys
            }
        }
    }

    // Don't forget the last submodule
    if let (Some(name), Some(path), Some(url)) = (current_name, current_path, current_url) {
        result.insert(
            path.clone(),
            SubmoduleInfo {
                name,
                path,
                url,
                branch: current_branch,
            },
        );
    }

    result
}

/// Find all submodule entries in a git tree.
///
/// Submodules appear in the tree with mode `160000` (commit mode).
///
/// # Arguments
///
/// * `repo` - The repository containing the tree
/// * `commit_id` - The commit whose tree to search
/// * `gitmodules` - Parsed `.gitmodules` info (path -> `SubmoduleInfo`)
///
/// # Returns
///
/// A list of submodule entries found in the tree.
///
/// # Errors
///
/// Returns `Git2Error` if the commit or tree cannot be found.
#[allow(clippy::implicit_hasher)] // We only use std HashMap internally
pub fn find_submodule_entries(
    repo: &Repository,
    commit_id: Oid,
    gitmodules: &HashMap<String, SubmoduleInfo>,
) -> Result<Vec<SubmoduleEntry>, Git2Error> {
    let commit = repo
        .find_commit(commit_id)
        .map_err(|e| Git2Error::Git2(format!("failed to find commit: {e}")))?;

    let tree = commit
        .tree()
        .map_err(|e| Git2Error::Git2(format!("failed to get tree: {e}")))?;

    let mut entries = Vec::new();

    tree.walk(TreeWalkMode::PreOrder, |dir, entry| {
        // Check if this is a submodule entry (mode 160000)
        if entry.filemode() == SUBMODULE_MODE && entry.kind() == Some(ObjectType::Commit) {
            let Some(name) = entry.name() else {
                return TreeWalkResult::Ok;
            };

            let path = if dir.is_empty() {
                name.to_string()
            } else {
                format!("{dir}{name}")
            };

            // Look up URL from gitmodules
            if let Some(info) = gitmodules.get(&path) {
                trace!(path = %path, commit = %entry.id(), "found submodule entry");
                entries.push(SubmoduleEntry {
                    path,
                    commit: entry.id(),
                    url: info.url.clone(),
                });
            } else {
                warn!(path = %path, "submodule entry found but not in .gitmodules");
            }
        }

        TreeWalkResult::Ok
    })
    .map_err(|e| Git2Error::Git2(format!("failed to walk tree: {e}")))?;

    debug!(count = entries.len(), "found submodule entries");

    Ok(entries)
}

/// Get the `.gitmodules` file content from a tree.
///
/// # Arguments
///
/// * `repo` - The repository
/// * `commit_id` - The commit to read from
///
/// # Returns
///
/// The raw content of `.gitmodules`, or `None` if not present.
#[must_use]
pub fn get_gitmodules_content(repo: &Repository, commit_id: Oid) -> Option<Vec<u8>> {
    let commit = repo.find_commit(commit_id).ok()?;
    let tree = commit.tree().ok()?;

    // Look for .gitmodules in the root
    let entry = tree.get_name(".gitmodules")?;

    if entry.kind() != Some(ObjectType::Blob) {
        return None;
    }

    let blob = repo.find_blob(entry.id()).ok()?;
    Some(blob.content().to_vec())
}

/// Fetch a single submodule.
///
/// # Arguments
///
/// * `entry` - The submodule entry to fetch
///
/// # Returns
///
/// The fetched submodule with its repository.
///
/// # Errors
///
/// Returns `Git2Error` if URL validation or fetch fails.
pub fn fetch_submodule(entry: &SubmoduleEntry) -> Result<FetchedSubmodule, Git2Error> {
    debug!(
        url = %sanitize_url_for_logging(&entry.url),
        path = %entry.path,
        commit = %entry.commit,
        "fetching submodule"
    );

    // Validate URL
    validate_url(&entry.url)?;

    // Fetch the submodule at the specific commit
    // We don't specify a branch since we want a specific commit
    let fetch_opts = FetchOptions2 {
        branch: None,
        depth: None,
        progress: None, // Submodule fetch progress is reported at higher level
    };

    let fetch_result = fetch_bare(&entry.url, Some(fetch_opts))?;

    // Verify the expected commit exists
    if fetch_result.repo.find_commit(entry.commit).is_err() {
        return Err(Git2Error::Git2(format!(
            "submodule commit {} not found in fetched repo",
            entry.commit
        )));
    }

    Ok(FetchedSubmodule {
        entry: entry.clone(),
        fetch_result,
    })
}

/// Fetch all submodules for a repository.
///
/// # Arguments
///
/// * `repo` - The parent repository (bare)
/// * `commit_id` - The commit whose submodules to fetch
///
/// # Returns
///
/// A list of successfully fetched submodules. Failed submodules are logged but skipped.
///
/// # Errors
///
/// Returns `Git2Error` if reading the tree fails.
pub fn fetch_all_submodules(
    repo: &Repository,
    commit_id: Oid,
) -> Result<Vec<FetchedSubmodule>, Git2Error> {
    // Get .gitmodules content
    let Some(gitmodules_content) = get_gitmodules_content(repo, commit_id) else {
        debug!("no .gitmodules found, no submodules to fetch");
        return Ok(Vec::new());
    };

    // Parse .gitmodules
    let gitmodules = parse_gitmodules(&gitmodules_content);
    if gitmodules.is_empty() {
        debug!(".gitmodules is empty or invalid");
        return Ok(Vec::new());
    }

    debug!(count = gitmodules.len(), "parsed .gitmodules");

    // Find submodule entries in tree
    let entries = find_submodule_entries(repo, commit_id, &gitmodules)?;
    let total_entries = entries.len();

    // Fetch each submodule
    let mut fetched = Vec::new();
    for entry in entries {
        match fetch_submodule(&entry) {
            Ok(submodule) => {
                fetched.push(submodule);
            }
            Err(e) => {
                warn!(
                    path = %entry.path,
                    url = %sanitize_url_for_logging(&entry.url),
                    error = %e,
                    "failed to fetch submodule, skipping"
                );
            }
        }
    }

    debug!(
        total = total_entries,
        fetched = fetched.len(),
        "submodule fetch complete"
    );

    Ok(fetched)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_gitmodules_basic() {
        let content = br#"[submodule "lib/foo"]
    path = lib/foo
    url = https://github.com/owner/foo.git

[submodule "vendor/bar"]
    path = vendor/bar
    url = git@github.com:owner/bar.git
    branch = develop
"#;

        let result = parse_gitmodules(content);
        assert_eq!(result.len(), 2);

        let foo = result.get("lib/foo").unwrap();
        assert_eq!(foo.name, "lib/foo");
        assert_eq!(foo.path, "lib/foo");
        assert_eq!(foo.url, "https://github.com/owner/foo.git");
        assert!(foo.branch.is_none());

        let bar = result.get("vendor/bar").unwrap();
        assert_eq!(bar.name, "vendor/bar");
        assert_eq!(bar.path, "vendor/bar");
        assert_eq!(bar.url, "git@github.com:owner/bar.git");
        assert_eq!(bar.branch, Some("develop".to_string()));
    }

    #[test]
    fn parse_gitmodules_with_comments() {
        let content = br#"# This is a comment
[submodule "lib"]
    ; Another comment
    path = lib
    url = https://example.com/lib.git
"#;

        let result = parse_gitmodules(content);
        assert_eq!(result.len(), 1);

        let lib = result.get("lib").unwrap();
        assert_eq!(lib.name, "lib");
        assert_eq!(lib.url, "https://example.com/lib.git");
    }

    #[test]
    fn parse_gitmodules_empty() {
        let content = b"";
        let result = parse_gitmodules(content);
        assert!(result.is_empty());
    }

    #[test]
    fn parse_gitmodules_invalid_utf8() {
        let content = &[0xff, 0xfe, 0x00, 0x01];
        let result = parse_gitmodules(content);
        assert!(result.is_empty());
    }

    #[test]
    fn parse_gitmodules_missing_url() {
        let content = br#"[submodule "incomplete"]
    path = some/path
"#;

        let result = parse_gitmodules(content);
        // Should not include submodule without URL
        assert!(result.is_empty());
    }

    #[test]
    fn parse_gitmodules_whitespace_handling() {
        let content = br#"[submodule "spaced"]
    path   =   path/with/spaces
    url=https://example.com/repo.git
"#;

        let result = parse_gitmodules(content);
        assert_eq!(result.len(), 1);

        let spaced = result.get("path/with/spaces").unwrap();
        assert_eq!(spaced.path, "path/with/spaces");
        assert_eq!(spaced.url, "https://example.com/repo.git");
    }

    #[test]
    fn submodule_mode_constant() {
        // Verify the octal mode is correct
        assert_eq!(SUBMODULE_MODE, 0o160_000);
        assert_eq!(SUBMODULE_MODE, 57344); // decimal equivalent
    }
}
