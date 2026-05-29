//! Handler for the `repo_clone` MCP tool.
//!
//! This tool clones a repository and returns its contents as a base64-encoded
//! tar.gz archive. The entire operation happens without writing source files
//! to the user's disk.
//!
//! # Data Flow
//!
//! ```text
//! 1. Fetch to bare repo (temp dir, git objects only)
//! 2. Walk tree, read blobs from object DB
//! 3. Build tar.gz in memory
//! 4. Base64 encode and return
//! 5. Temp dir auto-cleaned
//! ```
//!
//! # Security
//!
//! - Uses credential callbacks (SSH agent, credential helpers)
//! - No source files written to disk
//! - No credentials in response

use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use crate::config::{LfsConfig, ProxyConfig, SubmoduleConfig};
use crate::git2_ops::auth::{get_credentials_for_url, sanitize_url_for_logging};
use crate::git2_ops::clone::{fetch_bare, FetchOptions2, FetchResult};
use crate::git2_ops::error::Git2Error;
use crate::mcp::ProgressSender;
use crate::streaming::tar::{create_tar_from_tree_with_options, encode_base64, TarOptions};

/// Arguments for the `repo_clone` tool.
#[derive(Debug, Clone, Deserialize)]
pub struct RepoCloneArgs {
    /// Repository URL (https:// or git@)
    pub url: String,

    /// Branch to clone (defaults to the remote's default branch)
    #[serde(default)]
    pub branch: Option<String>,

    /// Shallow clone depth (1 = only latest commit, None = full history)
    #[serde(default)]
    pub depth: Option<u32>,

    /// Sparse checkout paths — only include files matching these patterns
    #[serde(default)]
    pub sparse: Option<Vec<String>>,

    /// Exclude binary files (files with null bytes or mostly non-printable chars).
    /// Useful for AI code review where only source code is needed.
    #[serde(default)]
    pub exclude_binary: Option<bool>,

    /// Maximum file size in bytes. Files larger than this are skipped.
    /// Useful for excluding large generated files or assets.
    #[serde(default)]
    pub max_file_size: Option<usize>,

    /// Resolve Git LFS pointers to actual content.
    /// When enabled, LFS pointer files are replaced with their actual content.
    #[serde(default)]
    pub resolve_lfs: Option<bool>,

    /// Include submodule contents in the archive.
    /// When enabled, submodules are fetched and their files are included
    /// at their respective paths.
    #[serde(default)]
    pub include_submodules: Option<bool>,

    /// Maximum submodule recursion depth (1 = top-level only).
    /// Overrides the server default from submodule config.
    #[serde(default)]
    pub submodule_depth: Option<u32>,

    /// Glob patterns for submodule paths to include.
    /// Only submodules matching at least one pattern are fetched.
    #[serde(default)]
    pub submodule_include: Option<Vec<String>>,

    /// Glob patterns for submodule paths to exclude.
    /// Submodules matching any pattern are skipped. Exclusions take
    /// precedence over inclusions.
    #[serde(default)]
    pub submodule_exclude: Option<Vec<String>>,
}

/// Result of a successful `repo_clone` operation.
#[derive(Debug, Clone, Serialize)]
pub struct RepoCloneResult {
    /// Base64-encoded tar.gz archive of the repository
    pub archive: String,

    /// The commit SHA that was cloned
    pub commit: String,

    /// The branch that was cloned
    pub branch: String,

    /// Number of files in the archive
    pub file_count: usize,

    /// Size of the archive in bytes (before base64 encoding)
    pub archive_size: usize,

    /// Number of files skipped by sparse filter (if any)
    #[serde(skip_serializing_if = "is_zero")]
    pub skipped_by_filter: usize,

    /// Number of binary files skipped (when `exclude_binary` is true)
    #[serde(skip_serializing_if = "is_zero")]
    pub skipped_binary: usize,

    /// Number of files skipped due to size limit (when `max_file_size` is set)
    #[serde(skip_serializing_if = "is_zero")]
    pub skipped_too_large: usize,

    /// Number of files skipped because their path could not be encoded in a
    /// tar header (rare; e.g. a path containing a NUL byte). Long paths are
    /// archived via a GNU long-name entry, not skipped.
    #[serde(skip_serializing_if = "is_zero")]
    pub skipped_path_too_long: usize,

    /// Number of LFS pointers resolved (when `resolve_lfs` is true)
    #[serde(skip_serializing_if = "is_zero")]
    pub lfs_resolved: usize,

    /// Number of LFS pointers that failed to resolve
    #[serde(skip_serializing_if = "is_zero")]
    pub lfs_failed: usize,

    /// Number of submodules successfully included (when `include_submodules` is true)
    #[serde(skip_serializing_if = "is_zero")]
    pub submodules_included: usize,

    /// Number of submodules that failed to fetch
    #[serde(skip_serializing_if = "is_zero")]
    pub submodules_failed: usize,

    /// Hint for AI assistants on how to extract the archive
    pub hint: String,
}

/// Helper for `skip_serializing_if` — skip if value is zero.
#[allow(clippy::trivially_copy_pass_by_ref)] // serde requires &T for skip_serializing_if
#[allow(clippy::missing_const_for_fn)] // serde skip_serializing_if doesn't need const
fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// Error from `repo_clone` operation (safe for display).
#[derive(Debug)]
pub struct RepoCloneError {
    /// Error message (credential-safe)
    pub message: String,
}

impl std::fmt::Display for RepoCloneError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl From<Git2Error> for RepoCloneError {
    fn from(err: Git2Error) -> Self {
        Self {
            message: err.to_string(),
        }
    }
}

/// Handle the `repo_clone` tool call.
///
/// This function:
/// 1. Validates the URL
/// 2. Fetches into a bare repository (no working tree)
/// 3. Creates a tar.gz archive from the git tree in memory
/// 4. Returns the base64-encoded archive with metadata
///
/// # Arguments
///
/// - `args`: The tool arguments from the MCP request
///
/// # Returns
///
/// A `RepoCloneResult` with the archive and metadata, or an error.
///
/// # Errors
///
/// Returns `RepoCloneError` if:
/// - URL validation fails
/// - Fetch operation fails (auth, network, etc.)
/// - Tar creation fails
///
/// # Security
///
/// - Credentials are handled via git2 callbacks (never stored)
/// - Source files are never written to disk
/// - The archive is built entirely in memory
pub fn handle_repo_clone(
    args: RepoCloneArgs,
    proxy_config: &ProxyConfig,
    lfs_config: &LfsConfig,
    submodule_config: &SubmoduleConfig,
) -> Result<RepoCloneResult, RepoCloneError> {
    handle_repo_clone_with_progress(args, proxy_config, lfs_config, submodule_config, None)
}

/// Handle the `repo_clone` tool call with optional progress reporting.
///
/// Same as [`handle_repo_clone`], but accepts an optional progress sender for
/// real-time progress updates during long operations.
///
/// # Progress Updates
///
/// When a progress sender is provided, the following updates are sent:
/// - Transfer progress during fetch (bytes, objects)
/// - File processing progress during tar creation
/// - LFS download progress (if `resolve_lfs` is true)
/// - Submodule fetch progress (if `include_submodules` is true)
///
/// # Errors
///
/// Returns `RepoCloneError` if:
/// - URL validation fails
/// - Fetch operation fails (auth, network, etc.)
/// - Tar creation fails
pub fn handle_repo_clone_with_progress(
    args: RepoCloneArgs,
    proxy_config: &ProxyConfig,
    lfs_config: &LfsConfig,
    submodule_config: &SubmoduleConfig,
    progress: Option<ProgressSender>,
) -> Result<RepoCloneResult, RepoCloneError> {
    info!(
        url = %sanitize_url_for_logging(&args.url),
        branch = ?args.branch,
        "repo_clone tool called"
    );

    // Fetch into bare repository
    let fetch_opts = FetchOptions2 {
        branch: args.branch.clone(),
        depth: args.depth,
        progress: progress.clone(),
        proxy_url: proxy_config.url.clone(),
    };

    let fetch_result = fetch_bare(&args.url, Some(fetch_opts))?;

    build_clone_result(
        &fetch_result,
        args,
        proxy_config,
        lfs_config,
        submodule_config,
        progress,
    )
}

/// Build the [`RepoCloneResult`] from an already-fetched bare repository.
///
/// Split out from [`handle_repo_clone_with_progress`] so the post-fetch path
/// (LFS credential retrieval, submodule-config merge, in-memory tar creation,
/// base64 encoding and result assembly) can be exercised by unit tests against
/// a locally-created bare repo, without a real network fetch.
fn build_clone_result(
    fetch_result: &FetchResult,
    args: RepoCloneArgs,
    proxy_config: &ProxyConfig,
    lfs_config: &LfsConfig,
    submodule_config: &SubmoduleConfig,
    progress: Option<ProgressSender>,
) -> Result<RepoCloneResult, RepoCloneError> {
    debug!(
        commit = %fetch_result.head_commit,
        branch = %fetch_result.branch,
        "fetch complete, creating tar"
    );

    // Log info about optional features
    if let Some(depth) = args.depth {
        debug!(depth = depth, "shallow clone requested");
    }
    if let Some(ref sparse) = args.sparse {
        debug!(patterns = ?sparse, "sparse checkout requested");
    }
    if args.exclude_binary == Some(true) {
        debug!("binary file exclusion enabled");
    }
    if let Some(max_size) = args.max_file_size {
        debug!(max_size = max_size, "max file size limit set");
    }
    if args.include_submodules == Some(true) {
        debug!("submodule inclusion enabled");
    }

    // Get LFS credentials from git credential helper if LFS resolution is enabled
    // Credentials are retrieved on-demand from OS credential stores (macOS Keychain,
    // Windows Credential Manager, etc.) and NEVER sent to AI - they stay on user's PC.
    let resolve_lfs = args.resolve_lfs == Some(true);
    let lfs_credentials = if resolve_lfs {
        debug!("LFS enabled, retrieving credentials from OS credential store");
        get_credentials_for_url(&args.url)
    } else {
        None
    };

    // Build effective submodule config: merge per-request overrides with server defaults.
    let effective_sub_config =
        submodule_config.with_request_overrides(args.submodule_include, args.submodule_exclude);

    // Create tar.gz from tree (in memory), with optional filtering.
    // `repo_url` is only consumed when LFS resolution is enabled, so only set
    // it then (matching `repo_clone_start`).
    let tar_opts = TarOptions {
        sparse_patterns: args.sparse,
        exclude_binary: args.exclude_binary,
        max_file_size: args.max_file_size,
        resolve_lfs: args.resolve_lfs,
        repo_url: if resolve_lfs { Some(args.url) } else { None },
        lfs_credentials, // From OS credential store, NEVER sent to AI
        include_submodules: args.include_submodules,
        proxy_url: proxy_config.url.clone(),
        no_proxy: proxy_config.no_proxy.clone(),
        progress,
        lfs_config: Some(lfs_config.clone()),
        submodule_config: Some(effective_sub_config),
        submodule_depth: args.submodule_depth,
    };

    let tar_result = create_tar_from_tree_with_options(
        &fetch_result.repo,
        fetch_result.head_commit,
        Some(tar_opts),
    )?;

    debug!(
        file_count = tar_result.file_count,
        compressed_size = tar_result.data.len(),
        uncompressed_size = tar_result.uncompressed_size,
        skipped_by_filter = tar_result.skipped_by_filter,
        skipped_binary = tar_result.skipped_binary,
        skipped_too_large = tar_result.skipped_too_large,
        skipped_path_too_long = tar_result.skipped_path_too_long,
        lfs_resolved = tar_result.lfs_resolved,
        lfs_failed = tar_result.lfs_failed,
        submodules_included = tar_result.submodules_included,
        submodules_failed = tar_result.submodules_failed,
        "tar creation complete"
    );

    // Base64 encode
    let archive_base64 = encode_base64(&tar_result.data);

    info!(
        commit = %fetch_result.head_commit,
        branch = %fetch_result.branch,
        file_count = tar_result.file_count,
        archive_size = tar_result.data.len(),
        "repo_clone complete"
    );

    Ok(RepoCloneResult {
        archive: archive_base64,
        commit: fetch_result.head_commit.to_string(),
        branch: fetch_result.branch.clone(),
        file_count: tar_result.file_count,
        archive_size: tar_result.data.len(),
        skipped_by_filter: tar_result.skipped_by_filter,
        skipped_binary: tar_result.skipped_binary,
        skipped_too_large: tar_result.skipped_too_large,
        skipped_path_too_long: tar_result.skipped_path_too_long,
        lfs_resolved: tar_result.lfs_resolved,
        lfs_failed: tar_result.lfs_failed,
        submodules_included: tar_result.submodules_included,
        submodules_failed: tar_result.submodules_failed,
        hint: "Use helper_script tool to get git_proxy_helper.py, then: python git_proxy_helper.py extract <result.json> <output_dir>".to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::tools::run_with_debug_logs;

    #[test]
    fn repo_clone_args_defaults() {
        let json = r#"{"url": "https://github.com/owner/repo.git"}"#;
        let args: RepoCloneArgs = serde_json::from_str(json).unwrap();
        assert_eq!(args.url, "https://github.com/owner/repo.git");
        assert!(args.branch.is_none());
        assert!(args.depth.is_none());
        assert!(args.sparse.is_none());
    }

    #[test]
    fn repo_clone_args_with_branch() {
        let json = r#"{"url": "https://github.com/owner/repo.git", "branch": "develop"}"#;
        let args: RepoCloneArgs = serde_json::from_str(json).unwrap();
        assert_eq!(args.branch, Some("develop".to_string()));
    }

    #[test]
    fn repo_clone_result_serializes() {
        let result = RepoCloneResult {
            archive: "SGVsbG8=".to_string(),
            commit: "abc123".to_string(),
            branch: "main".to_string(),
            file_count: 10,
            archive_size: 1024,
            skipped_by_filter: 0,
            skipped_binary: 0,
            skipped_too_large: 0,
            skipped_path_too_long: 0,
            lfs_resolved: 0,
            lfs_failed: 0,
            submodules_included: 0,
            submodules_failed: 0,
            hint: "test hint".to_string(),
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"archive\":\"SGVsbG8=\""));
        assert!(json.contains("\"file_count\":10"));
        // Zero skipped counts should not be serialized
        assert!(!json.contains("skipped"));
        assert!(!json.contains("lfs"));
        assert!(!json.contains("submodules"));
    }

    #[test]
    fn repo_clone_result_serializes_skipped_counts() {
        let result = RepoCloneResult {
            archive: "SGVsbG8=".to_string(),
            commit: "abc123".to_string(),
            branch: "main".to_string(),
            file_count: 10,
            archive_size: 1024,
            skipped_by_filter: 5,
            skipped_binary: 3,
            skipped_too_large: 2,
            skipped_path_too_long: 1,
            lfs_resolved: 0,
            lfs_failed: 0,
            submodules_included: 0,
            submodules_failed: 0,
            hint: "test hint".to_string(),
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"skipped_by_filter\":5"));
        assert!(json.contains("\"skipped_binary\":3"));
        assert!(json.contains("\"skipped_too_large\":2"));
        assert!(json.contains("\"skipped_path_too_long\":1"));
    }

    #[test]
    fn repo_clone_result_serializes_lfs_counts() {
        let result = RepoCloneResult {
            archive: "SGVsbG8=".to_string(),
            commit: "abc123".to_string(),
            branch: "main".to_string(),
            file_count: 10,
            archive_size: 1024,
            skipped_by_filter: 0,
            skipped_binary: 0,
            skipped_too_large: 0,
            skipped_path_too_long: 0,
            lfs_resolved: 3,
            lfs_failed: 1,
            submodules_included: 0,
            submodules_failed: 0,
            hint: "test hint".to_string(),
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"lfs_resolved\":3"));
        assert!(json.contains("\"lfs_failed\":1"));
    }

    #[test]
    fn repo_clone_result_serializes_submodule_counts() {
        let result = RepoCloneResult {
            archive: "SGVsbG8=".to_string(),
            commit: "abc123".to_string(),
            branch: "main".to_string(),
            file_count: 10,
            archive_size: 1024,
            skipped_by_filter: 0,
            skipped_binary: 0,
            skipped_too_large: 0,
            skipped_path_too_long: 0,
            lfs_resolved: 0,
            lfs_failed: 0,
            submodules_included: 2,
            submodules_failed: 1,
            hint: "test hint".to_string(),
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"submodules_included\":2"));
        assert!(json.contains("\"submodules_failed\":1"));
    }

    #[test]
    fn repo_clone_args_with_submodules() {
        let json = r#"{
            "url": "https://github.com/owner/repo.git",
            "include_submodules": true
        }"#;
        let args: RepoCloneArgs = serde_json::from_str(json).unwrap();
        assert_eq!(args.include_submodules, Some(true));
    }

    #[test]
    fn repo_clone_args_with_filtering() {
        let json = r#"{
            "url": "https://github.com/owner/repo.git",
            "exclude_binary": true,
            "max_file_size": 1048576
        }"#;
        let args: RepoCloneArgs = serde_json::from_str(json).unwrap();
        assert_eq!(args.exclude_binary, Some(true));
        assert_eq!(args.max_file_size, Some(1_048_576));
    }

    #[test]
    fn repo_clone_args_with_lfs() {
        let json = r#"{
            "url": "https://github.com/owner/repo.git",
            "resolve_lfs": true
        }"#;
        let args: RepoCloneArgs = serde_json::from_str(json).unwrap();
        assert_eq!(args.resolve_lfs, Some(true));
    }

    #[test]
    fn repo_clone_args_rejects_missing_url() {
        let json = "{}";
        let result: Result<RepoCloneArgs, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn repo_clone_args_with_submodule_options() {
        let json = r#"{
            "url": "https://github.com/owner/repo.git",
            "include_submodules": true,
            "submodule_depth": 2,
            "submodule_include": ["vendor/*"],
            "submodule_exclude": ["vendor/old/*"]
        }"#;
        let args: RepoCloneArgs = serde_json::from_str(json).unwrap();
        assert_eq!(args.include_submodules, Some(true));
        assert_eq!(args.submodule_depth, Some(2));
        assert_eq!(args.submodule_include.as_ref().unwrap().len(), 1);
        assert_eq!(args.submodule_exclude.as_ref().unwrap().len(), 1);
    }

    #[test]
    fn repo_clone_args_with_sparse_patterns() {
        let json = r#"{
            "url": "https://github.com/owner/repo.git",
            "sparse": ["src/**/*.rs", "*.md"]
        }"#;
        let args: RepoCloneArgs = serde_json::from_str(json).unwrap();
        let patterns = args.sparse.unwrap();
        assert_eq!(patterns.len(), 2);
        assert!(patterns.contains(&"src/**/*.rs".to_string()));
    }

    #[test]
    fn repo_clone_args_with_depth() {
        let json = r#"{
            "url": "https://github.com/owner/repo.git",
            "depth": 1
        }"#;
        let args: RepoCloneArgs = serde_json::from_str(json).unwrap();
        assert_eq!(args.depth, Some(1));
    }

    #[test]
    fn repo_clone_error_displays() {
        let err = RepoCloneError {
            message: "test error".to_string(),
        };
        assert_eq!(format!("{err}"), "test error");
    }

    #[test]
    fn repo_clone_error_from_git2_error() {
        let git2_err = Git2Error::InvalidUrl;
        let err: RepoCloneError = git2_err.into();
        assert!(err.message.contains("invalid"));
    }

    #[test]
    fn handle_repo_clone_with_invalid_url() {
        let args = RepoCloneArgs {
            url: "not-a-url".to_string(),
            branch: None,
            depth: None,
            sparse: None,
            exclude_binary: None,
            max_file_size: None,
            resolve_lfs: None,
            include_submodules: None,
            submodule_depth: None,
            submodule_include: None,
            submodule_exclude: None,
        };
        let proxy = ProxyConfig::default();
        let lfs = LfsConfig::default();
        let submods = SubmoduleConfig::default();
        assert!(handle_repo_clone(args, &proxy, &lfs, &submods).is_err());
    }

    #[test]
    fn handle_repo_clone_rejects_file_url() {
        let args = RepoCloneArgs {
            url: "file:///etc/passwd".to_string(),
            branch: None,
            depth: None,
            sparse: None,
            exclude_binary: None,
            max_file_size: None,
            resolve_lfs: None,
            include_submodules: None,
            submodule_depth: None,
            submodule_include: None,
            submodule_exclude: None,
        };
        let proxy = ProxyConfig::default();
        let lfs = LfsConfig::default();
        let submods = SubmoduleConfig::default();
        assert!(handle_repo_clone(args, &proxy, &lfs, &submods).is_err());
    }

    #[test]
    fn is_zero_helper() {
        assert!(is_zero(&0));
        assert!(!is_zero(&1));
        assert!(!is_zero(&100));
    }

    /// Build a `FetchResult` around a locally-created bare repo (no network)
    /// so the post-fetch `build_clone_result` path can be exercised directly.
    /// The repo has two files: `README.md` and `src/main.rs`.
    fn local_fetch_result() -> FetchResult {
        use git2::Repository;
        let temp = tempfile::TempDir::new().unwrap();
        let commit = {
            let repo = Repository::init_bare(temp.path()).unwrap();
            let readme = repo.blob(b"# Test Repo\n").unwrap();
            let main_rs = repo.blob(b"fn main() {}\n").unwrap();
            let mut tb = repo.treebuilder(None).unwrap();
            tb.insert("README.md", readme, 0o100_644).unwrap();
            let src = {
                let mut sb = repo.treebuilder(None).unwrap();
                sb.insert("main.rs", main_rs, 0o100_644).unwrap();
                sb.write().unwrap()
            };
            tb.insert("src", src, 0o040_000).unwrap();
            let tree_oid = tb.write().unwrap();
            let sig = git2::Signature::now("Test", "test@example.com").unwrap();
            let tree = repo.find_tree(tree_oid).unwrap();
            repo.commit(Some("HEAD"), &sig, &sig, "test", &tree, &[])
                .unwrap()
        };
        let repo = Repository::open_bare(temp.path()).unwrap();
        FetchResult::from_parts_for_test(repo, commit, "main".to_string(), temp)
    }

    fn clone_args(url: &str) -> RepoCloneArgs {
        RepoCloneArgs {
            url: url.to_string(),
            branch: None,
            depth: None,
            sparse: None,
            exclude_binary: None,
            max_file_size: None,
            resolve_lfs: None,
            include_submodules: None,
            submodule_depth: None,
            submodule_include: None,
            submodule_exclude: None,
        }
    }

    #[test]
    fn build_clone_result_archives_local_repo() {
        let fetch = local_fetch_result();
        let expected_commit = fetch.head_commit.to_string();
        let result = build_clone_result(
            &fetch,
            clone_args("https://github.com/owner/repo.git"),
            &ProxyConfig::default(),
            &LfsConfig::default(),
            &SubmoduleConfig::default(),
            None,
        )
        .unwrap();

        assert_eq!(result.commit, expected_commit);
        assert_eq!(result.branch, "main");
        assert_eq!(result.file_count, 2); // README.md + src/main.rs
        assert!(!result.archive.is_empty());
        assert!(result.archive_size > 0);
        assert_eq!(result.skipped_by_filter, 0);
    }

    #[test]
    fn build_clone_result_applies_sparse_and_logs_options() {
        // Exercises the optional-feature debug branches (depth, sparse,
        // exclude_binary, max_file_size, include_submodules) and the sparse
        // filter: only `src/main.rs` matches `**/*.rs`, so `README.md` is
        // counted as skipped.
        let fetch = local_fetch_result();
        let mut args = clone_args("https://github.com/owner/repo.git");
        args.depth = Some(1);
        args.sparse = Some(vec!["**/*.rs".to_string()]);
        args.exclude_binary = Some(true);
        args.max_file_size = Some(1_048_576);
        args.include_submodules = Some(true);
        args.submodule_include = Some(vec!["lib/*".to_string()]);

        // Run under a live subscriber so the per-feature `debug!` lines and the
        // completion `info!` are evaluated.
        let result = run_with_debug_logs(|| {
            build_clone_result(
                &fetch,
                args,
                &ProxyConfig::default(),
                &LfsConfig::default(),
                &SubmoduleConfig::default(),
                None,
            )
        })
        .unwrap();

        assert_eq!(result.file_count, 1);
        assert_eq!(result.skipped_by_filter, 1);
        assert_eq!(result.submodules_included, 0); // repo has no submodules
    }

    #[test]
    fn handle_repo_clone_emits_entry_log_then_fails_on_invalid_url() {
        // Drives the outer handler under a live subscriber so its entry
        // `info!` (which formats the sanitised URL) is evaluated; the invalid
        // URL then fails fast at the fetch with no network access.
        let result = run_with_debug_logs(|| {
            handle_repo_clone(
                clone_args("not-a-url"),
                &ProxyConfig::default(),
                &LfsConfig::default(),
                &SubmoduleConfig::default(),
            )
        });
        assert!(result.is_err());
    }

    #[test]
    fn build_clone_result_with_lfs_resolution_archives_non_pointer_files() {
        // Exercises the `resolve_lfs == Some(true)` branch: credentials are
        // retrieved (gracefully None when no helper is configured — and the
        // call degrades cleanly if git is absent), `repo_url` is set, and an
        // LFS client is created. The repo has no LFS pointers, so no network
        // request is made and the files are archived verbatim.
        let fetch = local_fetch_result();
        let mut args = clone_args("https://github.com/owner/repo.git");
        args.resolve_lfs = Some(true);

        let result = build_clone_result(
            &fetch,
            args,
            &ProxyConfig::default(),
            &LfsConfig::default(),
            &SubmoduleConfig::default(),
            None,
        )
        .unwrap();

        assert_eq!(result.file_count, 2);
        assert_eq!(result.lfs_resolved, 0);
        assert_eq!(result.lfs_failed, 0);
    }

    #[test]
    fn build_clone_result_propagates_tar_error_for_unknown_commit() {
        // A `FetchResult` pointing at a commit that does not exist in the repo
        // makes `create_tar_from_tree_with_options` fail; the error must
        // propagate as a `RepoCloneError` (covers the `?` error path and the
        // `Git2Error -> RepoCloneError` conversion).
        use git2::Repository;
        let temp = tempfile::TempDir::new().unwrap();
        Repository::init_bare(temp.path()).unwrap();
        let repo = Repository::open_bare(temp.path()).unwrap();
        let bogus = git2::Oid::from_str("dead00000000000000000000000000000000beef").unwrap();
        let fetch = FetchResult::from_parts_for_test(repo, bogus, "main".to_string(), temp);

        let result = build_clone_result(
            &fetch,
            clone_args("https://github.com/owner/repo.git"),
            &ProxyConfig::default(),
            &LfsConfig::default(),
            &SubmoduleConfig::default(),
            None,
        );
        assert!(result.is_err());
    }
}
