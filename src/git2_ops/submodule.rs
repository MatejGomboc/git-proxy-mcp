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
use std::collections::{HashMap, HashSet};
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
    /// Recursively fetched child submodules (if any).
    pub children: Vec<Self>,
}

/// Filter for submodule paths based on include/exclude glob patterns.
///
/// Exclude patterns take precedence over include patterns.
/// If no include patterns are set, all paths match (unless excluded).
pub struct SubmoduleFilter {
    /// Compiled include patterns.
    include: Vec<glob::Pattern>,
    /// Compiled exclude patterns.
    exclude: Vec<glob::Pattern>,
}

impl SubmoduleFilter {
    /// Creates a new filter from optional include and exclude pattern slices.
    ///
    /// Invalid patterns are logged and skipped.
    #[must_use]
    pub fn new(include: Option<&[String]>, exclude: Option<&[String]>) -> Self {
        let include = include
            .unwrap_or(&[])
            .iter()
            .filter_map(|p| match glob::Pattern::new(p) {
                Ok(pat) => Some(pat),
                Err(e) => {
                    warn!(pattern = %p, error = %e, "invalid submodule include pattern, skipping");
                    None
                }
            })
            .collect();

        let exclude = exclude
            .unwrap_or(&[])
            .iter()
            .filter_map(|p| match glob::Pattern::new(p) {
                Ok(pat) => Some(pat),
                Err(e) => {
                    warn!(pattern = %p, error = %e, "invalid submodule exclude pattern, skipping");
                    None
                }
            })
            .collect();

        Self { include, exclude }
    }

    /// Returns `true` if the given path passes the filter.
    ///
    /// A path is accepted if:
    /// 1. It does NOT match any exclude pattern, AND
    /// 2. Either no include patterns are set, or it matches at least one.
    #[must_use]
    pub fn matches(&self, path: &str) -> bool {
        // Exclude takes precedence
        if self.exclude.iter().any(|p| p.matches(path)) {
            return false;
        }

        // If no include patterns, everything matches
        if self.include.is_empty() {
            return true;
        }

        // Must match at least one include pattern
        self.include.iter().any(|p| p.matches(path))
    }
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
pub fn fetch_submodule(
    entry: &SubmoduleEntry,
    proxy_url: Option<&str>,
) -> Result<FetchedSubmodule, Git2Error> {
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
        proxy_url: proxy_url.map(String::from),
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
        children: Vec::new(),
    })
}

/// Fetch all submodules for a repository.
///
/// Delegates to the internal recursive fetcher with the provided configuration.
///
/// # Arguments
///
/// * `repo` - The parent repository (bare)
/// * `commit_id` - The commit whose submodules to fetch
/// * `proxy_url` - Optional proxy URL for network operations
/// * `max_depth` - Maximum recursion depth (1 = top-level only)
/// * `max_failures` - Maximum number of failures before stopping
/// * `filter` - Filter for include/exclude patterns
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
    proxy_url: Option<&str>,
    max_depth: u32,
    max_failures: usize,
    filter: &SubmoduleFilter,
) -> Result<Vec<FetchedSubmodule>, Git2Error> {
    let mut visited_urls: HashSet<String> = HashSet::new();
    let mut failure_count: usize = 0;

    fetch_submodules_recursive(
        repo,
        commit_id,
        proxy_url,
        1, // current depth starts at 1
        max_depth,
        max_failures,
        filter,
        &mut visited_urls,
        &mut failure_count,
    )
}

/// Normalise a URL for cycle detection.
///
/// Strips trailing `.git` suffix and trailing slashes so that
/// `https://example.com/repo.git` and `https://example.com/repo` are
/// treated as the same repository.
fn normalise_url_for_cycle_detection(url: &str) -> String {
    let mut normalised = url.to_lowercase();
    normalised = normalised.trim_end_matches('/').to_string();
    if std::path::Path::new(&normalised)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("git"))
    {
        // Remove the ".git" suffix (4 bytes)
        normalised.truncate(normalised.len() - 4);
    }
    normalised
}

/// Recursively fetch submodules up to a given depth.
///
/// This function:
/// 1. Reads `.gitmodules` from the given commit's tree
/// 2. Filters entries through the `SubmoduleFilter`
/// 3. Tracks failure count and stops early when `max_failures` is reached
/// 4. Uses a `HashSet` of normalised URLs to prevent cycles
/// 5. For each successful fetch, recursively fetches child submodules
///    if `current_depth < max_depth`
///
/// # Errors
///
/// Returns `Git2Error` if reading the tree fails.
#[allow(clippy::too_many_arguments)]
fn fetch_submodules_recursive(
    repo: &Repository,
    commit_id: Oid,
    proxy_url: Option<&str>,
    current_depth: u32,
    max_depth: u32,
    max_failures: usize,
    filter: &SubmoduleFilter,
    visited_urls: &mut HashSet<String>,
    failure_count: &mut usize,
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

    debug!(
        count = gitmodules.len(),
        depth = current_depth,
        "parsed .gitmodules"
    );

    // Find submodule entries in tree
    let entries = find_submodule_entries(repo, commit_id, &gitmodules)?;
    let total_entries = entries.len();

    // Fetch each submodule
    let mut fetched = Vec::new();
    for entry in entries {
        // Check max_failures early termination
        if *failure_count >= max_failures {
            warn!(
                failure_count = *failure_count,
                max_failures = max_failures,
                "max submodule failures reached, skipping remaining submodules"
            );
            break;
        }

        // Check filter
        if !filter.matches(&entry.path) {
            debug!(path = %entry.path, "submodule excluded by filter");
            continue;
        }

        // Check for URL cycles
        let normalised = normalise_url_for_cycle_detection(&entry.url);
        if !visited_urls.insert(normalised.clone()) {
            warn!(
                path = %entry.path,
                url = %sanitize_url_for_logging(&entry.url),
                "submodule URL cycle detected, skipping"
            );
            continue;
        }

        match fetch_submodule(&entry, proxy_url) {
            Ok(mut submodule) => {
                // Recursively fetch child submodules if depth allows
                if current_depth < max_depth {
                    match fetch_submodules_recursive(
                        &submodule.fetch_result.repo,
                        submodule.entry.commit,
                        proxy_url,
                        current_depth + 1,
                        max_depth,
                        max_failures,
                        filter,
                        visited_urls,
                        failure_count,
                    ) {
                        Ok(children) => {
                            submodule.children = children;
                        }
                        Err(e) => {
                            warn!(
                                path = %entry.path,
                                error = %e,
                                "failed to fetch child submodules"
                            );
                            // Child submodule tree failure does not count as
                            // a failure of this submodule itself.
                        }
                    }
                }

                fetched.push(submodule);
            }
            Err(e) => {
                *failure_count += 1;
                warn!(
                    path = %entry.path,
                    url = %sanitize_url_for_logging(&entry.url),
                    error = %e,
                    failure_count = *failure_count,
                    "failed to fetch submodule, skipping"
                );
            }
        }
    }

    debug!(
        total = total_entries,
        fetched = fetched.len(),
        depth = current_depth,
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

    // =========================================================================
    // SubmoduleFilter tests
    // =========================================================================

    #[test]
    fn filter_empty_matches_all() {
        let filter = SubmoduleFilter::new(None, None);
        assert!(filter.matches("lib/foo"));
        assert!(filter.matches("vendor/bar"));
        assert!(filter.matches("anything"));
    }

    #[test]
    fn filter_include_only() {
        let include = vec!["lib/*".to_string(), "deps/core".to_string()];
        let filter = SubmoduleFilter::new(Some(&include), None);
        assert!(filter.matches("lib/foo"));
        assert!(filter.matches("lib/bar"));
        assert!(filter.matches("deps/core"));
        assert!(!filter.matches("vendor/bar"));
        assert!(!filter.matches("deps/other"));
    }

    #[test]
    fn filter_exclude_only() {
        let exclude = vec!["vendor/*".to_string()];
        let filter = SubmoduleFilter::new(None, Some(&exclude));
        assert!(filter.matches("lib/foo"));
        assert!(filter.matches("deps/core"));
        assert!(!filter.matches("vendor/bar"));
        assert!(!filter.matches("vendor/something"));
    }

    #[test]
    fn filter_exclude_takes_precedence() {
        let include = vec!["lib/*".to_string(), "vendor/*".to_string()];
        let exclude = vec!["vendor/*".to_string()];
        let filter = SubmoduleFilter::new(Some(&include), Some(&exclude));

        assert!(filter.matches("lib/foo"));
        // vendor/* is in include but also in exclude — exclude wins
        assert!(!filter.matches("vendor/bar"));
    }

    #[test]
    fn filter_invalid_pattern_skipped() {
        let include = vec!["[invalid".to_string(), "lib/*".to_string()];
        let filter = SubmoduleFilter::new(Some(&include), None);
        // The invalid pattern is skipped; "lib/*" still works
        assert!(filter.matches("lib/foo"));
        assert!(!filter.matches("src/main"));
    }

    #[test]
    fn filter_empty_slices() {
        let include: Vec<String> = vec![];
        let exclude: Vec<String> = vec![];
        let filter = SubmoduleFilter::new(Some(&include), Some(&exclude));
        // Empty include = match all, empty exclude = exclude nothing
        assert!(filter.matches("anything"));
    }

    // =========================================================================
    // URL normalisation / cycle detection tests
    // =========================================================================

    #[test]
    fn normalise_url_strips_git_suffix() {
        assert_eq!(
            normalise_url_for_cycle_detection("https://github.com/owner/repo.git"),
            "https://github.com/owner/repo"
        );
    }

    #[test]
    fn normalise_url_strips_trailing_slash() {
        assert_eq!(
            normalise_url_for_cycle_detection("https://github.com/owner/repo/"),
            "https://github.com/owner/repo"
        );
    }

    #[test]
    fn normalise_url_case_insensitive() {
        assert_eq!(
            normalise_url_for_cycle_detection("https://GitHub.COM/Owner/Repo.git"),
            normalise_url_for_cycle_detection("https://github.com/owner/repo")
        );
    }

    #[test]
    fn cycle_detection_via_visited_set() {
        let mut visited = HashSet::new();

        let url_a = "https://github.com/owner/repo.git";
        let url_b = "https://github.com/owner/repo";

        let norm_a = normalise_url_for_cycle_detection(url_a);
        let norm_b = normalise_url_for_cycle_detection(url_b);

        assert!(visited.insert(norm_a));
        // Same repo with different URL form — should be detected as a cycle
        assert!(!visited.insert(norm_b));
    }
}
