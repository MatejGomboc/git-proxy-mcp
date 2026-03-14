//! Create tar.gz archives from git trees in memory.
//!
//! This module walks a git tree and creates a compressed tar archive
//! containing all files, without ever writing source files to disk.
//!
//! # How It Works
//!
//! 1. Walk the git tree recursively
//! 2. For each blob (file), read content from git object database
//! 3. Filter by sparse patterns (if provided)
//! 4. Append to tar archive (in memory)
//! 5. Compress with gzip
//! 6. Return raw bytes (for base64 encoding)
//!
//! # Sparse Checkout
//!
//! Supports filtering files by glob patterns (e.g., `src/**/*.rs`).
//! Only files matching at least one pattern are included.
//!
//! # Memory Usage
//!
//! Tier 1: O(repository size) — entire archive buffered in memory.
//! This is acceptable for small-to-medium repos.

use flate2::write::GzEncoder;
use flate2::Compression;
use git2::{ObjectType, Oid, Repository, TreeWalkMode, TreeWalkResult};
use glob::{MatchOptions, Pattern};
use tracing::{debug, trace, warn};

use crate::git2_ops::error::Git2Error;
use crate::git2_ops::lfs::{is_lfs_pointer, parse_lfs_pointer, LfsClient};
use crate::git2_ops::submodule::fetch_all_submodules;
use crate::mcp::ProgressSender;

/// Options for tar creation.
#[derive(Debug, Clone, Default)]
pub struct TarOptions {
    /// Sparse checkout patterns — only include files matching these glob patterns.
    /// If empty or None, all files are included.
    pub sparse_patterns: Option<Vec<String>>,

    /// Exclude binary files (files containing null bytes or mostly non-printable chars).
    /// Useful for AI code review where only source code is needed.
    pub exclude_binary: Option<bool>,

    /// Maximum file size in bytes. Files larger than this are skipped.
    /// Useful for excluding large generated files or assets.
    pub max_file_size: Option<usize>,

    /// Resolve Git LFS pointers to actual content.
    /// When enabled, LFS pointer files are replaced with their actual content.
    pub resolve_lfs: Option<bool>,

    /// Repository URL for LFS server discovery.
    /// Required when `resolve_lfs` is true.
    pub repo_url: Option<String>,

    /// LFS credentials (username, password) for authentication.
    /// If None, public LFS servers are accessed without auth.
    pub lfs_credentials: Option<(String, String)>,

    /// Include submodule contents in the archive.
    /// When enabled, submodules are fetched and their files are included
    /// at their respective paths in the archive.
    pub include_submodules: Option<bool>,

    /// Optional proxy URL for network operations (None = auto-detect from environment).
    pub proxy_url: Option<String>,

    /// Comma-separated list of hosts that should bypass the proxy.
    pub no_proxy: Option<String>,

    /// Optional progress sender for real-time updates during tar creation.
    pub progress: Option<ProgressSender>,
}

/// Compiled sparse patterns for efficient matching.
struct SparseFilter {
    patterns: Vec<Pattern>,
    /// Match options: `*` doesn't match path separators, case sensitive.
    match_options: MatchOptions,
}

impl SparseFilter {
    /// Create a filter from string patterns.
    fn new(patterns: &[String]) -> Self {
        let compiled: Vec<Pattern> = patterns
            .iter()
            .filter_map(|p| match Pattern::new(p) {
                Ok(pattern) => Some(pattern),
                Err(e) => {
                    warn!(pattern = %p, error = %e, "invalid sparse pattern, skipping");
                    None
                }
            })
            .collect();

        // Use options where `*` doesn't match `/`, like Unix shells
        let match_options = MatchOptions {
            case_sensitive: true,
            require_literal_separator: true,
            require_literal_leading_dot: false,
        };

        Self {
            patterns: compiled,
            match_options,
        }
    }

    /// Check if a path matches any of the patterns.
    fn matches(&self, path: &str) -> bool {
        if self.patterns.is_empty() {
            return true; // No patterns = include all
        }

        self.patterns
            .iter()
            .any(|p| p.matches_with(path, self.match_options))
    }
}

/// Detect if file content is binary.
///
/// A file is considered binary if:
/// - It contains null bytes (common in compiled binaries, images, etc.)
/// - More than 30% of the first 8KB are non-printable characters
///
/// This heuristic is similar to what Git uses internally.
fn is_binary(content: &[u8]) -> bool {
    // Check first 8KB for performance on large files
    let check_len = content.len().min(8192);
    let sample = &content[..check_len];

    // Null bytes are a strong indicator of binary
    if sample.contains(&0) {
        return true;
    }

    // Count non-printable, non-whitespace characters
    let non_text_count = sample
        .iter()
        .filter(|&&b| {
            // Consider printable ASCII (32-126), tab, newline, carriage return as text
            !((32..=126).contains(&b) || b == b'\t' || b == b'\n' || b == b'\r')
        })
        .count();

    // If more than 30% non-text, consider it binary
    let threshold = check_len * 30 / 100;
    non_text_count > threshold
}

/// Result of creating a tar archive.
#[derive(Debug)]
pub struct TarResult {
    /// The compressed tar.gz data
    pub data: Vec<u8>,
    /// Number of files included
    pub file_count: usize,
    /// Total uncompressed size
    pub uncompressed_size: u64,
    /// Number of files skipped by sparse filter
    pub skipped_by_filter: usize,
    /// Number of binary files skipped (when `exclude_binary` is true)
    pub skipped_binary: usize,
    /// Number of files skipped due to size limit
    pub skipped_too_large: usize,
    /// Number of files skipped due to path being too long for tar header
    pub skipped_path_too_long: usize,
    /// Number of LFS pointers resolved (when `resolve_lfs` is true)
    pub lfs_resolved: usize,
    /// Number of LFS pointers that failed to resolve
    pub lfs_failed: usize,
    /// Number of submodules successfully included (when `include_submodules` is true)
    pub submodules_included: usize,
    /// Number of submodules that failed to fetch
    pub submodules_failed: usize,
}

/// Create a tar.gz archive from a git tree.
///
/// This reads file contents directly from the git object database
/// (via `repo.find_blob()`) and never creates a working tree.
///
/// # Arguments
///
/// - `repo`: The repository containing the tree
/// - `commit_id`: The commit whose tree to archive
///
/// # Returns
///
/// A `TarResult` containing the compressed archive and metadata.
///
/// # Errors
///
/// Returns `Git2Error::Git2` if:
/// - The commit cannot be found
/// - The tree cannot be retrieved
/// - The tree walk fails
/// - The tar archive cannot be finalized
///
/// # Memory
///
/// The entire archive is buffered in memory. For large repos,
/// consider using chunked streaming (Tier 2).
pub fn create_tar_from_tree(repo: &Repository, commit_id: Oid) -> Result<TarResult, Git2Error> {
    create_tar_from_tree_with_options(repo, commit_id, None)
}

/// Create a tar.gz archive from a git tree with options.
///
/// Like [`create_tar_from_tree`], but with support for sparse checkout
/// patterns and other options.
///
/// # Arguments
///
/// - `repo`: The repository containing the tree
/// - `commit_id`: The commit whose tree to archive
/// - `options`: Optional tar creation options (sparse patterns, etc.)
///
/// # Sparse Checkout
///
/// If `options.sparse_patterns` is set, only files matching at least one
/// of the glob patterns will be included in the archive. Patterns use
/// standard glob syntax:
///
/// - `src/**/*.rs` — all Rust files under src/
/// - `*.md` — all markdown files in root
/// - `docs/*` — all files directly in docs/
///
/// # Errors
///
/// Returns `Git2Error::Git2` if:
/// - The commit cannot be found
/// - The tree cannot be retrieved
/// - The tree walk fails
/// - The tar archive cannot be finalized
#[allow(clippy::too_many_lines)] // Tree walk + tar creation is naturally verbose
pub fn create_tar_from_tree_with_options(
    repo: &Repository,
    commit_id: Oid,
    options: Option<TarOptions>,
) -> Result<TarResult, Git2Error> {
    let options = options.unwrap_or_default();

    debug!(
        commit = %commit_id,
        sparse = ?options.sparse_patterns,
        exclude_binary = ?options.exclude_binary,
        max_file_size = ?options.max_file_size,
        resolve_lfs = ?options.resolve_lfs,
        include_submodules = ?options.include_submodules,
        "creating tar from tree"
    );

    let commit = repo
        .find_commit(commit_id)
        .map_err(|e| Git2Error::Git2(format!("failed to find commit: {e}")))?;

    let tree = commit
        .tree()
        .map_err(|e| Git2Error::Git2(format!("failed to get tree: {e}")))?;

    // Build sparse filter if patterns are provided
    let filter = options
        .sparse_patterns
        .as_ref()
        .map(|p| SparseFilter::new(p));

    // Get filtering options
    let exclude_binary = options.exclude_binary.unwrap_or(false);
    let max_file_size = options.max_file_size;
    let resolve_lfs = options.resolve_lfs.unwrap_or(false);

    // Create LFS client if needed
    let lfs_client = if resolve_lfs {
        if let Some(ref url) = options.repo_url {
            match LfsClient::new(
                url,
                options.lfs_credentials.clone(),
                options.proxy_url.as_deref(),
                options.no_proxy.as_deref(),
            ) {
                Ok(client) => Some(client),
                Err(e) => {
                    warn!(error = %e, "failed to create LFS client, LFS files won't be resolved");
                    None
                }
            }
        } else {
            warn!("resolve_lfs is true but no repo_url provided");
            None
        }
    } else {
        None
    };

    // Get submodule option
    let include_submodules = options.include_submodules.unwrap_or(false);

    // Get proxy URL for submodule fetching
    let proxy_url = options.proxy_url;

    // Get progress sender (moved, not cloned - caller no longer needs it)
    let progress = options.progress;

    let mut archive_buffer = Vec::new();
    let mut file_count = 0usize;
    let mut uncompressed_size = 0u64;
    let mut skipped_by_filter = 0usize;
    let mut skipped_binary = 0usize;
    let mut skipped_too_large = 0usize;
    let mut skipped_path_too_long = 0usize;
    let mut lfs_resolved = 0usize;
    let mut lfs_failed = 0usize;
    let mut submodules_included = 0usize;
    let mut submodules_failed = 0usize;

    {
        let encoder = GzEncoder::new(&mut archive_buffer, Compression::fast());
        let mut tar_builder = tar::Builder::new(encoder);

        // Walk the tree and add each blob to the tar
        tree.walk(TreeWalkMode::PreOrder, |dir, entry| {
            let Some(name) = entry.name() else {
                return TreeWalkResult::Skip;
            };

            // Build the full path
            let path = if dir.is_empty() {
                name.to_string()
            } else {
                format!("{dir}{name}")
            };

            // Only process blobs (files)
            if entry.kind() == Some(ObjectType::Blob) {
                // Check sparse filter
                if let Some(ref f) = filter {
                    if !f.matches(&path) {
                        trace!(path = %path, "skipped by sparse filter");
                        skipped_by_filter += 1;
                        return TreeWalkResult::Ok;
                    }
                }

                match repo.find_blob(entry.id()) {
                    Ok(blob) => {
                        let raw_content = blob.content();

                        // Check if this is an LFS pointer and resolve if enabled
                        // The nested if-let structure is clearer than map_or for this logic
                        #[allow(clippy::option_if_let_else)]
                        let (content, is_lfs): (std::borrow::Cow<'_, [u8]>, bool) =
                            if let Some(ref client) = lfs_client {
                                if is_lfs_pointer(raw_content) {
                                    if let Some(pointer) = parse_lfs_pointer(raw_content) {
                                        trace!(path = %path, oid = %pointer.oid, "resolving LFS pointer");
                                        // Report LFS download progress
                                        if let Some(ref sender) = progress {
                                            // Truncation is acceptable - only used for progress display
                                            #[allow(clippy::cast_possible_truncation)]
                                            let size_hint = pointer.size as usize;
                                            sender.send_lfs_progress(lfs_resolved, 0, Some(&path), 0, size_hint);
                                        }
                                        match client.fetch_content(&pointer) {
                                            Ok(lfs_content) => {
                                                lfs_resolved += 1;
                                                // Report LFS download complete
                                                if let Some(ref sender) = progress {
                                                    sender.send_lfs_progress(lfs_resolved, 0, Some(&path), lfs_content.len(), lfs_content.len());
                                                }
                                                (std::borrow::Cow::Owned(lfs_content), true)
                                            }
                                            Err(e) => {
                                                warn!(path = %path, error = %e, "failed to fetch LFS content");
                                                lfs_failed += 1;
                                                // Include the pointer file as-is
                                                (std::borrow::Cow::Borrowed(raw_content), false)
                                            }
                                        }
                                    } else {
                                        (std::borrow::Cow::Borrowed(raw_content), false)
                                    }
                                } else {
                                    (std::borrow::Cow::Borrowed(raw_content), false)
                                }
                            } else {
                                (std::borrow::Cow::Borrowed(raw_content), false)
                            };

                        // Check file size limit (use resolved size for LFS)
                        if let Some(max_size) = max_file_size {
                            if content.len() > max_size {
                                trace!(path = %path, size = content.len(), max = max_size, "skipped: too large");
                                skipped_too_large += 1;
                                return TreeWalkResult::Ok;
                            }
                        }

                        // Check if binary (skip for LFS since we already fetched it)
                        if exclude_binary && !is_lfs && is_binary(&content) {
                            trace!(path = %path, "skipped: binary file");
                            skipped_binary += 1;
                            return TreeWalkResult::Ok;
                        }

                        trace!(path = %path, size = content.len(), lfs = is_lfs, "adding file to tar");

                        // Create tar header
                        let mut header = tar::Header::new_gnu();
                        if header.set_path(&path).is_err() {
                            // Path too long for tar header (>100 chars without extension)
                            debug!(path = %path, "path too long for tar, skipping");
                            skipped_path_too_long += 1;
                            return TreeWalkResult::Ok;
                        }
                        header.set_size(content.len() as u64);
                        // filemode() returns i32, but negative modes are invalid
                        #[allow(clippy::cast_sign_loss)]
                        header.set_mode(entry.filemode() as u32);
                        header.set_cksum();

                        // Append to tar
                        if tar_builder.append(&header, content.as_ref()).is_err() {
                            debug!(path = %path, "failed to append to tar, skipping");
                            return TreeWalkResult::Ok;
                        }

                        file_count += 1;
                        uncompressed_size += content.len() as u64;

                        // Report progress for file processing
                        if let Some(ref sender) = progress {
                            sender.send_file_progress(file_count, 0, Some(&path));
                        }
                    }
                    Err(e) => {
                        debug!(path = %path, error = %e, "failed to read blob, skipping");
                    }
                }
            }

            TreeWalkResult::Ok
        })
        .map_err(|e| Git2Error::Git2(format!("tree walk failed: {e}")))?;

        // Process submodules if enabled
        if include_submodules {
            debug!("fetching submodules");
            match fetch_all_submodules(repo, commit_id, proxy_url.as_deref()) {
                Ok(submodules) => {
                    let total_submodules = submodules.len();
                    let mut processed_submodules = 0usize;
                    for submodule in submodules {
                        // Report submodule fetch progress
                        if let Some(ref sender) = progress {
                            sender.send_submodule_progress(
                                processed_submodules,
                                total_submodules,
                                Some(&submodule.entry.path),
                            );
                        }
                        let submodule_path = &submodule.entry.path;
                        let submodule_commit = submodule.entry.commit;
                        let submodule_repo = &submodule.fetch_result.repo;

                        debug!(
                            path = %submodule_path,
                            commit = %submodule_commit,
                            "adding submodule contents to tar"
                        );

                        // Get the submodule's tree
                        let submodule_tree = match submodule_repo.find_commit(submodule_commit) {
                            Ok(commit) => match commit.tree() {
                                Ok(tree) => tree,
                                Err(e) => {
                                    warn!(path = %submodule_path, error = %e, "failed to get submodule tree");
                                    submodules_failed += 1;
                                    continue;
                                }
                            },
                            Err(e) => {
                                warn!(path = %submodule_path, error = %e, "failed to find submodule commit");
                                submodules_failed += 1;
                                continue;
                            }
                        };

                        // Walk the submodule tree and add files
                        let submodule_prefix = format!("{submodule_path}/");
                        let mut submodule_files = 0usize;

                        let walk_result = submodule_tree.walk(TreeWalkMode::PreOrder, |dir, entry| {
                            let Some(name) = entry.name() else {
                                return TreeWalkResult::Skip;
                            };

                            // Build the full path with submodule prefix
                            let relative_path = if dir.is_empty() {
                                name.to_string()
                            } else {
                                format!("{dir}{name}")
                            };
                            let full_path = format!("{submodule_prefix}{relative_path}");

                            // Only process blobs (files)
                            if entry.kind() == Some(ObjectType::Blob) {
                                // Check sparse filter
                                if let Some(ref f) = filter {
                                    if !f.matches(&full_path) {
                                        trace!(path = %full_path, "submodule file skipped by sparse filter");
                                        skipped_by_filter += 1;
                                        return TreeWalkResult::Ok;
                                    }
                                }

                                match submodule_repo.find_blob(entry.id()) {
                                    Ok(blob) => {
                                        let content = blob.content();

                                        // Check file size limit
                                        if let Some(max_size) = max_file_size {
                                            if content.len() > max_size {
                                                trace!(path = %full_path, size = content.len(), "submodule file too large");
                                                skipped_too_large += 1;
                                                return TreeWalkResult::Ok;
                                            }
                                        }

                                        // Check if binary
                                        if exclude_binary && is_binary(content) {
                                            trace!(path = %full_path, "submodule binary file skipped");
                                            skipped_binary += 1;
                                            return TreeWalkResult::Ok;
                                        }

                                        trace!(path = %full_path, size = content.len(), "adding submodule file to tar");

                                        // Create tar header
                                        let mut header = tar::Header::new_gnu();
                                        if header.set_path(&full_path).is_err() {
                                            debug!(path = %full_path, "submodule path too long for tar");
                                            skipped_path_too_long += 1;
                                            return TreeWalkResult::Ok;
                                        }
                                        header.set_size(content.len() as u64);
                                        #[allow(clippy::cast_sign_loss)]
                                        header.set_mode(entry.filemode() as u32);
                                        header.set_cksum();

                                        if tar_builder.append(&header, content).is_err() {
                                            debug!(path = %full_path, "failed to append submodule file");
                                            return TreeWalkResult::Ok;
                                        }

                                        submodule_files += 1;
                                        file_count += 1;
                                        uncompressed_size += content.len() as u64;
                                    }
                                    Err(e) => {
                                        debug!(path = %full_path, error = %e, "failed to read submodule blob");
                                    }
                                }
                            }

                            TreeWalkResult::Ok
                        });

                        if walk_result.is_ok() && submodule_files > 0 {
                            debug!(
                                path = %submodule_path,
                                files = submodule_files,
                                "submodule added to tar"
                            );
                            submodules_included += 1;
                            processed_submodules += 1;
                        } else if walk_result.is_err() {
                            warn!(path = %submodule_path, "failed to walk submodule tree");
                            submodules_failed += 1;
                            processed_submodules += 1;
                        }
                    }

                    debug!(
                        total = total_submodules,
                        included = submodules_included,
                        failed = submodules_failed,
                        "submodule processing complete"
                    );
                }
                Err(e) => {
                    warn!(error = %e, "failed to fetch submodules");
                }
            }
        }

        // Finish the tar archive
        tar_builder
            .finish()
            .map_err(|e| Git2Error::Git2(format!("failed to finish tar: {e}")))?;
    }

    debug!(
        file_count = file_count,
        skipped_by_filter = skipped_by_filter,
        skipped_binary = skipped_binary,
        skipped_too_large = skipped_too_large,
        skipped_path_too_long = skipped_path_too_long,
        lfs_resolved = lfs_resolved,
        lfs_failed = lfs_failed,
        submodules_included = submodules_included,
        submodules_failed = submodules_failed,
        uncompressed_size = uncompressed_size,
        compressed_size = archive_buffer.len(),
        "tar creation complete"
    );

    Ok(TarResult {
        data: archive_buffer,
        file_count,
        uncompressed_size,
        skipped_by_filter,
        skipped_binary,
        skipped_too_large,
        skipped_path_too_long,
        lfs_resolved,
        lfs_failed,
        submodules_included,
        submodules_failed,
    })
}

/// Encode tar data as base64 for MCP response.
#[must_use]
pub fn encode_base64(data: &[u8]) -> String {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;
    STANDARD.encode(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_base64_works() {
        let data = b"hello world";
        let encoded = encode_base64(data);
        assert_eq!(encoded, "aGVsbG8gd29ybGQ=");
    }

    #[test]
    fn sparse_filter_empty_matches_all() {
        let filter = SparseFilter::new(&[]);
        assert!(filter.matches("any/path/file.rs"));
        assert!(filter.matches("README.md"));
    }

    #[test]
    fn sparse_filter_single_pattern() {
        let filter = SparseFilter::new(&["*.rs".to_string()]);
        assert!(filter.matches("main.rs"));
        assert!(filter.matches("lib.rs"));
        assert!(!filter.matches("README.md"));
        assert!(!filter.matches("src/main.rs")); // glob doesn't match path separators with *
    }

    #[test]
    fn sparse_filter_glob_star() {
        let filter = SparseFilter::new(&["src/**/*.rs".to_string()]);
        assert!(filter.matches("src/main.rs"));
        assert!(filter.matches("src/lib/mod.rs"));
        assert!(filter.matches("src/a/b/c/d.rs"));
        assert!(!filter.matches("main.rs"));
        assert!(!filter.matches("tests/test.rs"));
    }

    #[test]
    fn sparse_filter_multiple_patterns() {
        let filter = SparseFilter::new(&["*.md".to_string(), "src/**/*.rs".to_string()]);
        assert!(filter.matches("README.md"));
        assert!(filter.matches("CHANGELOG.md"));
        assert!(filter.matches("src/main.rs"));
        assert!(!filter.matches("Cargo.toml"));
        assert!(!filter.matches("tests/test.rs"));
    }

    #[test]
    fn sparse_filter_invalid_pattern_skipped() {
        // Invalid pattern should be skipped but not crash
        let filter = SparseFilter::new(&["[invalid".to_string(), "*.rs".to_string()]);
        assert!(filter.matches("main.rs"));
        assert!(!filter.matches("README.md"));
    }

    #[test]
    fn tar_options_default() {
        let opts = TarOptions::default();
        assert!(opts.sparse_patterns.is_none());
        assert!(opts.exclude_binary.is_none());
        assert!(opts.max_file_size.is_none());
    }

    #[test]
    fn is_binary_detects_null_bytes() {
        // Binary file with null bytes
        let binary = b"some\x00binary\x00content";
        assert!(is_binary(binary));
    }

    #[test]
    fn is_binary_accepts_text() {
        // Plain text file
        let text = b"Hello, World!\nThis is a text file.\n";
        assert!(!is_binary(text));
    }

    #[test]
    fn is_binary_accepts_source_code() {
        // Rust source code
        let code = b"fn main() {\n    println!(\"Hello\");\n}\n";
        assert!(!is_binary(code));
    }

    #[test]
    fn is_binary_detects_high_non_printable() {
        // File with many non-printable characters (>30%)
        let mut data = vec![0x80u8; 50];
        data.extend_from_slice(b"some text");
        assert!(is_binary(&data));
    }

    #[test]
    fn is_binary_accepts_utf8() {
        // UTF-8 text with non-ASCII characters should be treated as binary
        // (our simple heuristic doesn't handle UTF-8 specially)
        let _utf8 = "Hello 世界".as_bytes();
        // UTF-8 multibyte chars have bytes > 127, counted as non-printable
        // This is a known limitation - we err on the side of caution
        // For small amounts of UTF-8, it should still pass
        let text_with_some_utf8 = b"Hello world with a few UTF-8: \xc3\xa9";
        // Less than 30% non-printable, should pass
        assert!(!is_binary(text_with_some_utf8));
    }

    // Integration tests that require a git repo are in tests/streaming_tests.rs
}
