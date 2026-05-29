//! Handler for the `repo_clone_start` MCP tool (Tier 2).
//!
//! This tool initiates a chunked clone operation for large repositories.
//! It fetches the repository and creates a streaming session, returning
//! session info that the AI can use to retrieve chunks.
//!
//! # Protocol
//!
//! ```text
//! 1. AI calls repo_clone_start with URL, branch, chunk_size, and options
//! 2. Server fetches repo, creates tar.gz, creates streaming session
//! 3. Server returns session_id, total_chunks, total_size, and statistics
//! 4. AI calls repo_clone_chunk repeatedly to get data
//! ```
//!
//! # Features
//!
//! This tool supports all the same options as `repo_clone`:
//! - Sparse checkout patterns (`sparse`)
//! - Binary file exclusion (`exclude_binary`)
//! - File size limits (`max_file_size`)
//! - LFS resolution (`resolve_lfs`)
//! - Submodule inclusion (`include_submodules`)
//!
//! # Memory Model
//!
//! For archives larger than 10 MiB, data is stored in a temp file instead
//! of memory (disk-backed storage). The benefits are:
//! - O(chunk size) memory instead of O(archive size)
//! - Progress tracking via chunk retrieval
//! - Resume on failure
//! - Smaller individual responses

use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use crate::config::{LfsConfig, ProxyConfig, SubmoduleConfig};
use crate::git2_ops::auth::{get_credentials_for_url, sanitize_url_for_logging};
use crate::git2_ops::clone::{fetch_bare, FetchOptions2, FetchResult};
use crate::git2_ops::error::Git2Error;
use crate::streaming::chunked::{
    StreamingError, StreamingSessionManager, DEFAULT_CHUNK_SIZE, MAX_CHUNK_SIZE,
};
use crate::streaming::tar::{create_tar_from_tree_with_options, TarOptions};

/// Arguments for the `repo_clone_start` tool.
#[derive(Debug, Clone, Deserialize)]
pub struct RepoCloneStartArgs {
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

    /// Chunk size in bytes (default: 1 MiB, range: 1 KiB – 4 MiB after clamping).
    #[serde(default)]
    pub chunk_size: Option<usize>,

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

/// Result of a successful `repo_clone_start` operation.
#[derive(Debug, Clone, Serialize)]
pub struct RepoCloneStartResult {
    /// Session ID for subsequent chunk requests
    pub session_id: String,

    /// Total number of chunks to retrieve
    pub total_chunks: usize,

    /// Total size of the archive in bytes
    pub total_size: usize,

    /// Size of each chunk in bytes
    pub chunk_size: usize,

    /// The commit SHA that was cloned
    pub commit: String,

    /// The branch that was cloned
    pub branch: String,

    /// Number of files in the archive
    pub file_count: usize,

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

    /// Hint for AI assistants on how to handle chunked results
    pub hint: String,
}

/// Helper for `skip_serializing_if` — skip if value is zero.
#[allow(clippy::trivially_copy_pass_by_ref)] // serde requires &T for skip_serializing_if
#[allow(clippy::missing_const_for_fn)] // serde skip_serializing_if doesn't need const
fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// Error from `repo_clone_start` operation (safe for display).
#[derive(Debug)]
pub struct RepoCloneStartError {
    /// Error message (credential-safe)
    pub message: String,
}

impl std::fmt::Display for RepoCloneStartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl From<Git2Error> for RepoCloneStartError {
    fn from(err: Git2Error) -> Self {
        Self {
            message: err.to_string(),
        }
    }
}

impl From<StreamingError> for RepoCloneStartError {
    fn from(err: StreamingError) -> Self {
        Self {
            message: err.to_string(),
        }
    }
}

/// Clamp a requested chunk size to the supported range.
///
/// `None` selects [`DEFAULT_CHUNK_SIZE`]; any larger request is capped at
/// [`MAX_CHUNK_SIZE`]. The lower bound (1 KiB) is enforced later by the
/// streaming session when the session is created.
fn resolve_chunk_size(requested: Option<usize>) -> usize {
    requested.map_or(DEFAULT_CHUNK_SIZE, |s| s.min(MAX_CHUNK_SIZE))
}

/// Handle the `repo_clone_start` tool call.
///
/// This function:
/// 1. Validates the URL
/// 2. Fetches into a bare repository (no working tree)
/// 3. Creates a tar.gz archive from the git tree in memory
/// 4. Creates a streaming session for chunked retrieval
/// 5. Returns session info for chunk requests
///
/// # Arguments
///
/// - `args`: The tool arguments from the MCP request
/// - `session_manager`: The streaming session manager
///
/// # Returns
///
/// A `RepoCloneStartResult` with session info, or an error.
///
/// # Errors
///
/// Returns `RepoCloneStartError` if:
/// - URL validation fails
/// - Fetch operation fails (auth, network, etc.)
/// - Tar creation fails
/// - Session creation fails (too many active sessions)
pub fn handle_repo_clone_start(
    args: RepoCloneStartArgs,
    proxy_config: &ProxyConfig,
    lfs_config: &LfsConfig,
    submodule_config: &SubmoduleConfig,
    session_manager: &StreamingSessionManager,
) -> Result<RepoCloneStartResult, RepoCloneStartError> {
    info!(
        url = %sanitize_url_for_logging(&args.url),
        branch = ?args.branch,
        chunk_size = ?args.chunk_size,
        "repo_clone_start tool called"
    );

    // Fetch into bare repository
    let fetch_opts = FetchOptions2 {
        branch: args.branch.clone(),
        depth: args.depth,
        progress: None,
        proxy_url: proxy_config.url.clone(),
    };

    let fetch_result = fetch_bare(&args.url, Some(fetch_opts))?;

    build_clone_start_result(
        &fetch_result,
        args,
        proxy_config,
        lfs_config,
        submodule_config,
        session_manager,
    )
}

/// Build the [`RepoCloneStartResult`] from an already-fetched bare repository.
///
/// Split out from [`handle_repo_clone_start`] so the post-fetch path (LFS
/// credential retrieval, submodule-config merge, in-memory tar creation,
/// streaming-session creation and result assembly) can be exercised by unit
/// tests against a locally-created bare repo, without a real network fetch.
fn build_clone_start_result(
    fetch_result: &FetchResult,
    args: RepoCloneStartArgs,
    proxy_config: &ProxyConfig,
    lfs_config: &LfsConfig,
    submodule_config: &SubmoduleConfig,
    session_manager: &StreamingSessionManager,
) -> Result<RepoCloneStartResult, RepoCloneStartError> {
    debug!(
        commit = %fetch_result.head_commit,
        branch = %fetch_result.branch,
        "fetch complete, creating tar"
    );

    // Sanitise the URL now, before `args.url` may be moved into the tar
    // options below. The streaming-session key uses this sanitised form so a
    // credential embedded in the URL is never stored.
    let sanitized_url = sanitize_url_for_logging(&args.url);

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

    let chunk_size = resolve_chunk_size(args.chunk_size);

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
    // `repo_url` is only consumed when LFS resolution is enabled.
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
        progress: None,
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
        "tar creation complete, creating streaming session"
    );

    // Create streaming session.
    let session_info = session_manager.create_session(
        &sanitized_url,
        &fetch_result.branch,
        &fetch_result.head_commit.to_string(),
        tar_result.data,
        chunk_size,
    )?;

    info!(
        session_id = %session_info.session_id,
        total_chunks = session_info.total_chunks,
        total_size = session_info.total_size,
        chunk_size = session_info.chunk_size,
        file_count = tar_result.file_count,
        "repo_clone_start complete"
    );

    Ok(RepoCloneStartResult {
        session_id: session_info.session_id,
        total_chunks: session_info.total_chunks,
        total_size: session_info.total_size,
        chunk_size: session_info.chunk_size,
        commit: session_info.commit,
        branch: session_info.branch,
        file_count: tar_result.file_count,
        skipped_by_filter: tar_result.skipped_by_filter,
        skipped_binary: tar_result.skipped_binary,
        skipped_too_large: tar_result.skipped_too_large,
        skipped_path_too_long: tar_result.skipped_path_too_long,
        lfs_resolved: tar_result.lfs_resolved,
        lfs_failed: tar_result.lfs_failed,
        submodules_included: tar_result.submodules_included,
        submodules_failed: tar_result.submodules_failed,
        hint: "Use repo_clone_chunk to get all chunks, concatenate, then use helper_script tool for extraction".to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_clone_start_args_defaults() {
        let json = r#"{"url": "https://github.com/owner/repo.git"}"#;
        let args: RepoCloneStartArgs = serde_json::from_str(json).unwrap();
        assert_eq!(args.url, "https://github.com/owner/repo.git");
        assert!(args.branch.is_none());
        assert!(args.depth.is_none());
        assert!(args.sparse.is_none());
        assert!(args.chunk_size.is_none());
        assert!(args.exclude_binary.is_none());
        assert!(args.max_file_size.is_none());
        assert!(args.resolve_lfs.is_none());
        assert!(args.include_submodules.is_none());
    }

    #[test]
    fn repo_clone_start_args_with_chunk_size() {
        let json = r#"{"url": "https://github.com/owner/repo.git", "chunk_size": 2097152}"#;
        let args: RepoCloneStartArgs = serde_json::from_str(json).unwrap();
        assert_eq!(args.chunk_size, Some(2_097_152));
    }

    #[test]
    fn repo_clone_start_args_with_all_options() {
        let json = r#"{
            "url": "https://github.com/owner/repo.git",
            "branch": "develop",
            "depth": 1,
            "sparse": ["src/**"],
            "chunk_size": 2097152,
            "exclude_binary": true,
            "max_file_size": 1048576,
            "resolve_lfs": true,
            "include_submodules": true
        }"#;
        let args: RepoCloneStartArgs = serde_json::from_str(json).unwrap();
        assert_eq!(args.branch, Some("develop".to_string()));
        assert_eq!(args.depth, Some(1));
        assert_eq!(args.sparse, Some(vec!["src/**".to_string()]));
        assert_eq!(args.chunk_size, Some(2_097_152));
        assert_eq!(args.exclude_binary, Some(true));
        assert_eq!(args.max_file_size, Some(1_048_576));
        assert_eq!(args.resolve_lfs, Some(true));
        assert_eq!(args.include_submodules, Some(true));
    }

    #[test]
    fn repo_clone_start_result_serializes() {
        let result = RepoCloneStartResult {
            session_id: "stream_abc123".to_string(),
            total_chunks: 10,
            total_size: 10240,
            chunk_size: 1024,
            commit: "abc123".to_string(),
            branch: "main".to_string(),
            file_count: 50,
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
        assert!(json.contains("\"session_id\":\"stream_abc123\""));
        assert!(json.contains("\"total_chunks\":10"));
        assert!(json.contains("\"file_count\":50"));
        // Zero skipped counts should not be serialized
        assert!(!json.contains("skipped"));
        assert!(!json.contains("lfs"));
        assert!(!json.contains("submodules"));
    }

    #[test]
    fn repo_clone_start_result_serializes_skipped_counts() {
        let result = RepoCloneStartResult {
            session_id: "stream_abc123".to_string(),
            total_chunks: 10,
            total_size: 10240,
            chunk_size: 1024,
            commit: "abc123".to_string(),
            branch: "main".to_string(),
            file_count: 50,
            skipped_by_filter: 5,
            skipped_binary: 3,
            skipped_too_large: 2,
            skipped_path_too_long: 1,
            lfs_resolved: 4,
            lfs_failed: 1,
            submodules_included: 2,
            submodules_failed: 1,
            hint: "test hint".to_string(),
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"skipped_by_filter\":5"));
        assert!(json.contains("\"skipped_binary\":3"));
        assert!(json.contains("\"skipped_too_large\":2"));
        assert!(json.contains("\"skipped_path_too_long\":1"));
        assert!(json.contains("\"lfs_resolved\":4"));
        assert!(json.contains("\"lfs_failed\":1"));
        assert!(json.contains("\"submodules_included\":2"));
        assert!(json.contains("\"submodules_failed\":1"));
    }

    #[test]
    fn repo_clone_start_args_rejects_missing_url() {
        let json = "{}";
        let result: Result<RepoCloneStartArgs, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn repo_clone_start_args_with_submodule_options() {
        let json = r#"{
            "url": "https://github.com/owner/repo.git",
            "submodule_depth": 3,
            "submodule_include": ["vendor/*"],
            "submodule_exclude": ["vendor/old/*"]
        }"#;
        let args: RepoCloneStartArgs = serde_json::from_str(json).unwrap();
        assert_eq!(args.submodule_depth, Some(3));
        assert_eq!(args.submodule_include.as_ref().unwrap().len(), 1);
    }

    #[test]
    fn repo_clone_start_error_displays() {
        let err = RepoCloneStartError {
            message: "test error".to_string(),
        };
        assert_eq!(format!("{err}"), "test error");
    }

    #[test]
    fn repo_clone_start_error_from_git2_error() {
        let git2_err = Git2Error::InvalidUrl;
        let err: RepoCloneStartError = git2_err.into();
        assert!(err.message.contains("invalid"));
    }

    #[test]
    fn handle_repo_clone_start_with_invalid_url() {
        let args = RepoCloneStartArgs {
            url: "not-a-url".to_string(),
            branch: None,
            depth: None,
            chunk_size: None,
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
        let manager = StreamingSessionManager::default();
        assert!(handle_repo_clone_start(args, &proxy, &lfs, &submods, &manager).is_err());
    }

    #[test]
    fn handle_repo_clone_start_rejects_file_url() {
        let args = RepoCloneStartArgs {
            url: "file:///etc/passwd".to_string(),
            branch: None,
            depth: None,
            chunk_size: None,
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
        let manager = StreamingSessionManager::default();
        assert!(handle_repo_clone_start(args, &proxy, &lfs, &submods, &manager).is_err());
    }

    #[test]
    fn resolve_chunk_size_clamps_and_defaults() {
        assert_eq!(resolve_chunk_size(None), DEFAULT_CHUNK_SIZE);
        assert_eq!(resolve_chunk_size(Some(1024)), 1024);
        assert_eq!(resolve_chunk_size(Some(MAX_CHUNK_SIZE)), MAX_CHUNK_SIZE);
        assert_eq!(resolve_chunk_size(Some(MAX_CHUNK_SIZE + 1)), MAX_CHUNK_SIZE);
        assert_eq!(resolve_chunk_size(Some(usize::MAX)), MAX_CHUNK_SIZE);
    }

    /// Run a closure with a DEBUG-level `tracing` subscriber active (output
    /// discarded), so the `debug!`/`info!` field expressions in the handlers
    /// are actually evaluated and counted as covered.
    fn run_with_debug_logs<T>(f: impl FnOnce() -> T) -> T {
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .with_writer(std::io::sink)
            .finish();
        tracing::subscriber::with_default(subscriber, f)
    }

    /// Build a `FetchResult` around a locally-created bare repo (no network)
    /// so the post-fetch `build_clone_start_result` path can be exercised
    /// directly. The repo has two files: `README.md` and `src/main.rs`.
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

    fn start_args(url: &str) -> RepoCloneStartArgs {
        RepoCloneStartArgs {
            url: url.to_string(),
            branch: None,
            depth: None,
            sparse: None,
            chunk_size: None,
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
    fn build_clone_start_result_creates_retrievable_session() {
        let fetch = local_fetch_result();
        let expected_commit = fetch.head_commit.to_string();
        let manager = StreamingSessionManager::default();

        let mut args = start_args("https://github.com/owner/repo.git");
        // Below the 1 KiB minimum; the session clamps it up to 1024.
        args.chunk_size = Some(64);

        let result = build_clone_start_result(
            &fetch,
            args,
            &ProxyConfig::default(),
            &LfsConfig::default(),
            &SubmoduleConfig::default(),
            &manager,
        )
        .unwrap();

        assert_eq!(result.commit, expected_commit);
        assert_eq!(result.branch, "main");
        assert_eq!(result.file_count, 2);
        assert_eq!(result.chunk_size, 1024); // clamped up from 64
        assert!(result.total_chunks >= 1);
        assert!(!result.session_id.is_empty());

        // The session is registered and its chunk count matches the result.
        let status = manager.get_session_status(&result.session_id).unwrap();
        assert_eq!(status.total_chunks, result.total_chunks);
        assert!(!status.is_complete);
    }

    #[test]
    fn build_clone_start_result_applies_sparse_and_logs_options() {
        // Exercises the optional-feature debug branches and the sparse filter.
        let fetch = local_fetch_result();
        let manager = StreamingSessionManager::default();

        let mut args = start_args("https://github.com/owner/repo.git");
        args.depth = Some(1);
        args.sparse = Some(vec!["**/*.rs".to_string()]);
        args.exclude_binary = Some(true);
        args.max_file_size = Some(1_048_576);
        args.include_submodules = Some(true);

        // Run under a live subscriber so the per-feature `debug!` lines and the
        // completion `info!` are evaluated.
        let result = run_with_debug_logs(|| {
            build_clone_start_result(
                &fetch,
                args,
                &ProxyConfig::default(),
                &LfsConfig::default(),
                &SubmoduleConfig::default(),
                &manager,
            )
        })
        .unwrap();

        assert_eq!(result.file_count, 1);
        assert_eq!(result.skipped_by_filter, 1);
    }

    #[test]
    fn build_clone_start_result_with_lfs_resolution() {
        // Exercises the `resolve_lfs == Some(true)` branch: credentials are
        // retrieved (gracefully None without a helper / git), `repo_url` is set
        // and an LFS client is created. The repo has no LFS pointers, so no
        // network request is made.
        let fetch = local_fetch_result();
        let manager = StreamingSessionManager::default();
        let mut args = start_args("https://github.com/owner/repo.git");
        args.resolve_lfs = Some(true);

        let result = build_clone_start_result(
            &fetch,
            args,
            &ProxyConfig::default(),
            &LfsConfig::default(),
            &SubmoduleConfig::default(),
            &manager,
        )
        .unwrap();

        assert_eq!(result.file_count, 2);
        assert_eq!(result.lfs_resolved, 0);
    }

    #[test]
    fn handle_repo_clone_start_emits_entry_log_then_fails_on_invalid_url() {
        // Drives the outer handler under a live subscriber so its entry
        // `info!` is evaluated; the invalid URL fails fast at the fetch.
        let manager = StreamingSessionManager::default();
        let result = run_with_debug_logs(|| {
            handle_repo_clone_start(
                start_args("not-a-url"),
                &ProxyConfig::default(),
                &LfsConfig::default(),
                &SubmoduleConfig::default(),
                &manager,
            )
        });
        assert!(result.is_err());
    }

    #[test]
    fn repo_clone_start_error_from_streaming_error() {
        // The `From<StreamingError>` conversion is used by `?` when session
        // creation fails (e.g. too many sessions).
        let err: RepoCloneStartError = StreamingError::TooManySessions { max: 3 }.into();
        assert!(err.message.contains('3') || !err.message.is_empty());
    }

    #[test]
    fn build_clone_start_result_propagates_tar_error_for_unknown_commit() {
        // A commit that does not exist makes tar creation fail; the error must
        // propagate (covers the `?` error path before session creation).
        use git2::Repository;
        let temp = tempfile::TempDir::new().unwrap();
        Repository::init_bare(temp.path()).unwrap();
        let repo = Repository::open_bare(temp.path()).unwrap();
        let bogus = git2::Oid::from_str("dead00000000000000000000000000000000beef").unwrap();
        let fetch = FetchResult::from_parts_for_test(repo, bogus, "main".to_string(), temp);
        let manager = StreamingSessionManager::default();

        let result = build_clone_start_result(
            &fetch,
            start_args("https://github.com/owner/repo.git"),
            &ProxyConfig::default(),
            &LfsConfig::default(),
            &SubmoduleConfig::default(),
            &manager,
        );
        assert!(result.is_err());
    }

    #[test]
    fn build_clone_start_result_propagates_session_error_when_full() {
        // A session manager with zero capacity makes `create_session` fail
        // after a successful tar build; the `StreamingError` must propagate as
        // a `RepoCloneStartError` (covers the session-creation `?` path).
        let fetch = local_fetch_result();
        let manager = StreamingSessionManager::new(std::time::Duration::from_secs(3600), 0);

        let result = build_clone_start_result(
            &fetch,
            start_args("https://github.com/owner/repo.git"),
            &ProxyConfig::default(),
            &LfsConfig::default(),
            &SubmoduleConfig::default(),
            &manager,
        );
        assert!(result.is_err());
    }
}
