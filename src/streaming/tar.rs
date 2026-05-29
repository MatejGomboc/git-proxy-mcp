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

use crate::config::{LfsConfig, SubmoduleConfig};
use crate::git2_ops::error::Git2Error;
use crate::git2_ops::lfs::{is_lfs_pointer, parse_lfs_pointer, LfsClient};
use crate::git2_ops::submodule::{fetch_all_submodules, FetchedSubmodule, SubmoduleFilter};
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

    /// Optional LFS configuration (retry behaviour, size limits).
    /// When `None`, defaults are used.
    pub lfs_config: Option<LfsConfig>,

    /// Optional submodule configuration (filtering, failure limits).
    /// When `None`, defaults are used.
    pub submodule_config: Option<SubmoduleConfig>,

    /// Submodule recursion depth.
    /// `None` = unlimited (git default). `0` = skip submodules.
    pub submodule_depth: Option<u32>,
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
/// - More than 30% of the first 8 KiB are non-printable characters
///
/// The null-byte check mirrors Git's own heuristic (Git scans the first
/// 8000 bytes for a NUL). The additional non-printable-ratio rule is ours
/// and is deliberately conservative — because UTF-8 multibyte sequences use
/// bytes ≥ 0x80, it can misclassify text that is mostly non-Latin (CJK,
/// Cyrillic, etc.) as binary, so callers that need such files should not set
/// `exclude_binary`.
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

/// Map a git tree-entry filemode to a tar header mode.
///
/// Git blob modes are `0o100644`/`0o100755` (regular files) and `0o120000`
/// (symlinks — stored as blobs whose content is the link target). We archive
/// every blob as a regular file, so only the permission bits are kept; a
/// symlink's `0o120000` has none and would mask to `0o000`, which extracts as
/// an unreadable file, so it falls back to `0o644`.
#[allow(clippy::cast_sign_loss)] // a git filemode is always a positive value
const fn tar_mode_for_filemode(filemode: i32) -> u32 {
    let perms = filemode as u32 & 0o777;
    if perms == 0 {
        0o644
    } else {
        perms
    }
}

/// Append one blob to the tar archive as a regular file at `path`, returning
/// whether it was written.
///
/// Builds the header (size + [`tar_mode_for_filemode`]) and uses
/// [`tar::Builder::append_data`], which writes a GNU long-name entry for paths
/// too long for the ustar `name` field — so long paths are archived, not
/// dropped. Returns `false` only if the path cannot be encoded at all (which a
/// git tree name can't trigger) or the underlying writer fails; in that case it
/// logs at debug, increments `skipped_path_too_long`, and returns `false` so
/// the caller skips the file without aborting the whole archive. Shared by the
/// main-tree and submodule walks.
fn append_blob_to_tar<W: std::io::Write>(
    tar_builder: &mut tar::Builder<W>,
    path: &str,
    content: &[u8],
    filemode: i32,
    skipped_path_too_long: &mut usize,
) -> bool {
    let mut header = tar::Header::new_gnu();
    header.set_size(content.len() as u64);
    header.set_mode(tar_mode_for_filemode(filemode));
    if let Err(e) = tar_builder.append_data(&mut header, path, content) {
        debug!(path = %path, error = %e, "failed to append file to tar, skipping");
        *skipped_path_too_long += 1;
        return false;
    }
    true
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
    /// Number of files skipped because their path could not be encoded in a
    /// tar header (e.g. it contained a NUL byte). Paths that are merely long
    /// are written via a GNU long-name entry, not skipped.
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
/// - The tar archive cannot be finalised
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
/// - The tar archive cannot be finalised
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

    // Get LFS config (use default if not provided)
    let lfs_config = options.lfs_config.clone().unwrap_or_default();

    // Create LFS client if needed
    let lfs_client = if resolve_lfs {
        if let Some(ref url) = options.repo_url {
            match LfsClient::new(
                url,
                options.lfs_credentials.clone(),
                options.proxy_url.as_deref(),
                options.no_proxy.as_deref(),
                &lfs_config,
                options.progress.clone(),
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
            // git2 0.21: `TreeEntry::name()` returns `Result` (UTF-8 check);
            // `Err` means a non-UTF-8 name, which we skip as before.
            let Ok(name) = entry.name() else {
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

                        // Append as a regular file via the shared helper, which
                        // writes a GNU long-name entry for over-long paths (so
                        // they aren't dropped) and records a skip only if the
                        // path can't be encoded at all.
                        if append_blob_to_tar(
                            &mut tar_builder,
                            &path,
                            content.as_ref(),
                            entry.filemode(),
                            &mut skipped_path_too_long,
                        ) {
                            file_count += 1;
                            uncompressed_size += content.len() as u64;

                            // Report progress for file processing
                            if let Some(ref sender) = progress {
                                sender.send_file_progress(file_count, 0, Some(&path));
                            }
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

        // Process submodules if enabled (submodule_depth=0 means skip submodules)
        if include_submodules {
            let sub_cfg = options.submodule_config.unwrap_or_default();
            let depth = options.submodule_depth.unwrap_or(u32::MAX);

            if depth == 0 {
                debug!("submodule_depth=0, skipping submodule fetching");
            } else {
                debug!("fetching submodules");
                let sub_filter = SubmoduleFilter::new(
                    sub_cfg.include_patterns.as_deref(),
                    sub_cfg.exclude_patterns.as_deref(),
                );

                match fetch_all_submodules(
                    repo,
                    commit_id,
                    proxy_url.as_deref(),
                    depth,
                    sub_cfg.max_failures,
                    sub_cfg.max_concurrent,
                    &sub_filter,
                ) {
                    Ok(submodules) => {
                        // Flatten and write all submodules (including children) to tar
                        write_submodules_to_tar(
                            &submodules,
                            "",
                            filter.as_ref(),
                            exclude_binary,
                            max_file_size,
                            &mut tar_builder,
                            &mut file_count,
                            &mut uncompressed_size,
                            &mut skipped_by_filter,
                            &mut skipped_binary,
                            &mut skipped_too_large,
                            &mut skipped_path_too_long,
                            &mut submodules_included,
                            &mut submodules_failed,
                            progress.as_ref(),
                        );
                    }
                    Err(e) => {
                        warn!(error = %e, "failed to fetch submodules");
                    }
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

/// Write fetched submodules (and their recursive children) into a tar builder.
///
/// This function walks each submodule's tree, applies filtering, and appends
/// blobs to the tar archive. It then recurses into each submodule's children.
#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_lines)] // Submodule tree walk + tar creation is naturally verbose
fn write_submodules_to_tar<W: std::io::Write>(
    submodules: &[FetchedSubmodule],
    parent_prefix: &str,
    filter: Option<&SparseFilter>,
    exclude_binary: bool,
    max_file_size: Option<usize>,
    tar_builder: &mut tar::Builder<W>,
    file_count: &mut usize,
    uncompressed_size: &mut u64,
    skipped_by_filter: &mut usize,
    skipped_binary: &mut usize,
    skipped_too_large: &mut usize,
    skipped_path_too_long: &mut usize,
    submodules_included: &mut usize,
    submodules_failed: &mut usize,
    progress: Option<&ProgressSender>,
) {
    let total_submodules = submodules.len();
    let mut processed_submodules = 0usize;

    for submodule in submodules {
        // Report submodule fetch progress
        if let Some(sender) = progress {
            sender.send_submodule_progress(
                processed_submodules,
                total_submodules,
                Some(&submodule.entry.path),
            );
        }

        let submodule_path = if parent_prefix.is_empty() {
            submodule.entry.path.clone()
        } else {
            format!("{parent_prefix}{}", submodule.entry.path)
        };
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
                    *submodules_failed += 1;
                    continue;
                }
            },
            Err(e) => {
                warn!(path = %submodule_path, error = %e, "failed to find submodule commit");
                *submodules_failed += 1;
                continue;
            }
        };

        // Walk the submodule tree and add files
        let submodule_prefix = format!("{submodule_path}/");
        let mut submodule_files = 0usize;

        let walk_result = submodule_tree.walk(TreeWalkMode::PreOrder, |dir, entry| {
            // git2 0.21: `TreeEntry::name()` returns `Result` (UTF-8 check);
            // `Err` means a non-UTF-8 name, which we skip as before.
            let Ok(name) = entry.name() else {
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
                if let Some(f) = filter {
                    if !f.matches(&full_path) {
                        trace!(path = %full_path, "submodule file skipped by sparse filter");
                        *skipped_by_filter += 1;
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
                                *skipped_too_large += 1;
                                return TreeWalkResult::Ok;
                            }
                        }

                        // Check if binary
                        if exclude_binary && is_binary(content) {
                            trace!(path = %full_path, "submodule binary file skipped");
                            *skipped_binary += 1;
                            return TreeWalkResult::Ok;
                        }

                        trace!(path = %full_path, size = content.len(), "adding submodule file to tar");

                        // Append via the shared helper (same long-name and
                        // skip handling as the main-tree walk).
                        if append_blob_to_tar(
                            tar_builder,
                            &full_path,
                            content,
                            entry.filemode(),
                            skipped_path_too_long,
                        ) {
                            submodule_files += 1;
                            *file_count += 1;
                            *uncompressed_size += content.len() as u64;
                        }
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
            *submodules_included += 1;
        } else if walk_result.is_err() {
            warn!(path = %submodule_path, "failed to walk submodule tree");
            *submodules_failed += 1;
        }

        // Advance the progress counter once per submodule regardless of
        // outcome — an empty or fully-filtered submodule (walk OK, zero
        // files) still counts as processed, so the submodule progress
        // percentage can reach 100%.
        processed_submodules += 1;

        // Recursively write child submodules
        if !submodule.children.is_empty() {
            write_submodules_to_tar(
                &submodule.children,
                &submodule_prefix,
                filter,
                exclude_binary,
                max_file_size,
                tar_builder,
                file_count,
                uncompressed_size,
                skipped_by_filter,
                skipped_binary,
                skipped_too_large,
                skipped_path_too_long,
                submodules_included,
                submodules_failed,
                progress,
            );
        }
    }
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
    fn write_submodules_to_tar_writes_submodule_files() {
        // Exercises `write_submodules_to_tar` (including its
        // `let Ok(name) = entry.name()` walk branch) without a network fetch,
        // by building a `FetchedSubmodule` around a locally-created bare repo.
        use crate::git2_ops::clone::FetchResult;
        use crate::git2_ops::submodule::SubmoduleEntry;

        let temp = tempfile::TempDir::new().unwrap();
        let commit_oid = {
            let repo = Repository::init_bare(temp.path()).unwrap();
            let blob = repo.blob(b"submodule file contents\n").unwrap();
            let mut tb = repo.treebuilder(None).unwrap();
            tb.insert("README.md", blob, 0o100_644).unwrap();
            let tree = repo.find_tree(tb.write().unwrap()).unwrap();
            let sig = git2::Signature::now("Test", "test@example.com").unwrap();
            repo.commit(None, &sig, &sig, "submodule commit", &tree, &[])
                .unwrap()
        };
        let repo = Repository::open_bare(temp.path()).unwrap();

        let fetched = FetchedSubmodule {
            entry: SubmoduleEntry {
                path: "vendor/sub".to_string(),
                commit: commit_oid,
                url: "https://example.com/sub.git".to_string(),
            },
            fetch_result: FetchResult::from_parts_for_test(
                repo,
                commit_oid,
                "main".to_string(),
                temp,
            ),
            children: Vec::new(),
        };

        let mut archive = Vec::new();
        {
            let encoder = GzEncoder::new(&mut archive, Compression::fast());
            let mut tar_builder = tar::Builder::new(encoder);

            let mut file_count = 0usize;
            let mut uncompressed_size = 0u64;
            let mut skipped_by_filter = 0usize;
            let mut skipped_binary = 0usize;
            let mut skipped_too_large = 0usize;
            let mut skipped_path_too_long = 0usize;
            let mut submodules_included = 0usize;
            let mut submodules_failed = 0usize;

            write_submodules_to_tar(
                std::slice::from_ref(&fetched),
                "",
                None,
                false,
                None,
                &mut tar_builder,
                &mut file_count,
                &mut uncompressed_size,
                &mut skipped_by_filter,
                &mut skipped_binary,
                &mut skipped_too_large,
                &mut skipped_path_too_long,
                &mut submodules_included,
                &mut submodules_failed,
                None,
            );

            tar_builder.finish().unwrap();

            assert_eq!(file_count, 1, "the submodule's single file should be added");
            assert_eq!(submodules_included, 1);
            assert_eq!(submodules_failed, 0);
            assert!(uncompressed_size > 0);
        }

        // The archive should contain the submodule file under its path prefix.
        assert!(!archive.is_empty());
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
        // UTF-8 multibyte chars have bytes ≥ 0x80, which the heuristic counts
        // as non-printable — so a file that is *mostly* non-Latin UTF-8 can be
        // misclassified as binary (a documented limitation). A predominantly
        // ASCII file with only a little UTF-8 stays under the 30% threshold and
        // is correctly treated as text.
        let text_with_some_utf8 = b"Hello world with a few UTF-8: \xc3\xa9";
        assert!(!is_binary(text_with_some_utf8));
    }

    #[test]
    fn is_binary_empty_input() {
        assert!(!is_binary(b""));
    }

    #[test]
    fn is_binary_only_null() {
        assert!(is_binary(&[0u8]));
    }

    #[test]
    fn is_binary_exactly_at_threshold() {
        // 30 non-text bytes out of 100 is exactly 30%. The implementation uses
        // `non_text_count > threshold` (strictly greater) with
        // threshold = 100 * 30 / 100 = 30, so 30 > 30 is false: a sample that
        // is exactly 30% non-text is classified as TEXT.
        let mut data = vec![0x80u8; 30];
        data.extend(vec![b'a'; 70]);
        assert!(
            !is_binary(&data),
            "exactly 30% non-text must be treated as text (count > threshold, not >=)"
        );
    }

    /// Helper: build a test bare repo and return the path + HEAD commit oid.
    fn build_test_repo() -> (tempfile::TempDir, Oid) {
        let temp = tempfile::TempDir::new().unwrap();
        let commit_oid = {
            let repo = Repository::init_bare(temp.path()).unwrap();

            let readme_oid = repo.blob(b"# Test Repo\n").unwrap();
            let main_rs_oid = repo.blob(b"fn main() {}\n").unwrap();
            // Binary blob with null bytes
            let bin_oid = repo.blob(&[0u8, 1, 2, 3, 0, 0, 0, 0]).unwrap();

            let mut tree_builder = repo.treebuilder(None).unwrap();
            tree_builder
                .insert("README.md", readme_oid, 0o100_644)
                .unwrap();

            let src_tree_oid = {
                let mut src_builder = repo.treebuilder(None).unwrap();
                src_builder
                    .insert("main.rs", main_rs_oid, 0o100_644)
                    .unwrap();
                src_builder.write().unwrap()
            };

            tree_builder.insert("src", src_tree_oid, 0o040_000).unwrap();
            tree_builder.insert("data.bin", bin_oid, 0o100_644).unwrap();
            let tree_oid = tree_builder.write().unwrap();

            let signature = git2::Signature::now("Test", "test@example.com").unwrap();
            let tree = repo.find_tree(tree_oid).unwrap();
            repo.commit(Some("HEAD"), &signature, &signature, "test", &tree, &[])
                .unwrap()
        };
        (temp, commit_oid)
    }

    fn open_test_repo(temp: &tempfile::TempDir) -> Repository {
        Repository::open_bare(temp.path()).unwrap()
    }

    #[test]
    fn create_tar_from_tree_basic() {
        let (temp, commit_oid) = build_test_repo();
        let repo = open_test_repo(&temp);
        let result = create_tar_from_tree(&repo, commit_oid).unwrap();
        assert!(result.file_count >= 3);
        assert!(result.uncompressed_size > 0);
        assert!(!result.data.is_empty());
    }

    #[test]
    fn create_tar_with_sparse_filter() {
        let (temp, commit_oid) = build_test_repo();
        let repo = open_test_repo(&temp);
        let opts = TarOptions {
            sparse_patterns: Some(vec!["*.md".to_string()]),
            ..Default::default()
        };
        let result = create_tar_from_tree_with_options(&repo, commit_oid, Some(opts)).unwrap();
        assert!(result.file_count >= 1);
        assert!(result.skipped_by_filter > 0);
    }

    #[test]
    fn create_tar_with_exclude_binary() {
        let (temp, commit_oid) = build_test_repo();
        let repo = open_test_repo(&temp);
        let opts = TarOptions {
            exclude_binary: Some(true),
            ..Default::default()
        };
        let result = create_tar_from_tree_with_options(&repo, commit_oid, Some(opts)).unwrap();
        assert!(result.skipped_binary >= 1);
    }

    #[test]
    fn create_tar_with_max_file_size() {
        let (temp, commit_oid) = build_test_repo();
        let repo = open_test_repo(&temp);
        let opts = TarOptions {
            max_file_size: Some(1),
            ..Default::default()
        };
        let result = create_tar_from_tree_with_options(&repo, commit_oid, Some(opts)).unwrap();
        assert!(result.skipped_too_large >= 1);
    }

    #[test]
    fn create_tar_combined_filters() {
        let (temp, commit_oid) = build_test_repo();
        let repo = open_test_repo(&temp);
        let opts = TarOptions {
            sparse_patterns: Some(vec!["**/*".to_string()]),
            exclude_binary: Some(true),
            max_file_size: Some(1024 * 1024),
            ..Default::default()
        };
        let result = create_tar_from_tree_with_options(&repo, commit_oid, Some(opts)).unwrap();
        assert!(result.file_count >= 2);
    }

    #[test]
    fn create_tar_empty_tree() {
        let temp = tempfile::TempDir::new().unwrap();
        let commit_oid = {
            let repo = Repository::init_bare(temp.path()).unwrap();
            let tree_oid = repo.treebuilder(None).unwrap().write().unwrap();
            let signature = git2::Signature::now("Test", "test@example.com").unwrap();
            let tree = repo.find_tree(tree_oid).unwrap();
            repo.commit(Some("HEAD"), &signature, &signature, "empty", &tree, &[])
                .unwrap()
        };
        let repo = open_test_repo(&temp);
        let result = create_tar_from_tree(&repo, commit_oid).unwrap();
        assert_eq!(result.file_count, 0);
    }

    #[test]
    fn create_tar_with_invalid_commit() {
        let temp = tempfile::TempDir::new().unwrap();
        let repo = Repository::init_bare(temp.path()).unwrap();
        let bogus_oid = Oid::from_str("0000000000000000000000000000000000000001").unwrap();
        let result = create_tar_from_tree(&repo, bogus_oid);
        assert!(result.is_err());
    }

    #[test]
    fn create_tar_with_non_matching_sparse_includes_nothing() {
        let (temp, commit_oid) = build_test_repo();
        let repo = open_test_repo(&temp);
        let opts = TarOptions {
            sparse_patterns: Some(vec!["nonexistent_pattern.xyz".to_string()]),
            ..Default::default()
        };
        let result = create_tar_from_tree_with_options(&repo, commit_oid, Some(opts)).unwrap();
        assert_eq!(result.file_count, 0);
        assert!(result.skipped_by_filter >= 3);
    }

    #[test]
    fn create_tar_lfs_disabled_by_default() {
        let (temp, commit_oid) = build_test_repo();
        let repo = open_test_repo(&temp);
        let result = create_tar_from_tree(&repo, commit_oid).unwrap();
        // No LFS resolution attempted
        assert_eq!(result.lfs_resolved, 0);
        assert_eq!(result.lfs_failed, 0);
    }

    #[test]
    fn tar_options_default_has_all_none() {
        let opts = TarOptions::default();
        assert!(opts.sparse_patterns.is_none());
        assert!(opts.exclude_binary.is_none());
        assert!(opts.max_file_size.is_none());
        assert!(opts.resolve_lfs.is_none());
        assert!(opts.include_submodules.is_none());
        assert!(opts.submodule_depth.is_none());
    }

    #[test]
    fn tar_options_clone_works() {
        let opts = TarOptions {
            sparse_patterns: Some(vec!["*.rs".into()]),
            exclude_binary: Some(true),
            max_file_size: Some(1024),
            ..Default::default()
        };
        let cloned = opts;
        assert_eq!(cloned.sparse_patterns.as_ref().unwrap().len(), 1);
        assert_eq!(cloned.exclude_binary, Some(true));
        assert_eq!(cloned.max_file_size, Some(1024));
    }

    #[test]
    fn encode_base64_empty() {
        assert_eq!(encode_base64(&[]), "");
    }

    #[test]
    fn encode_base64_single_byte() {
        let result = encode_base64(&[0xff]);
        assert_eq!(result, "/w==");
    }

    #[test]
    fn encode_base64_round_trip() {
        use base64::Engine;
        let original = b"hello, world!";
        let encoded = encode_base64(original);
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&encoded)
            .unwrap();
        assert_eq!(decoded, original);
    }

    /// Decode a gzip+tar archive and return its entry path strings.
    fn tar_entry_paths(gz: &[u8]) -> Vec<String> {
        use flate2::read::GzDecoder;
        let mut archive = tar::Archive::new(GzDecoder::new(gz));
        archive
            .entries()
            .unwrap()
            .filter_map(|e| {
                let entry = e.ok()?;
                let path = entry.path().ok()?;
                Some(path.to_string_lossy().into_owned())
            })
            .collect()
    }

    #[test]
    fn create_tar_includes_file_with_long_path() {
        // A single filename longer than the 100-byte ustar `name` field (and
        // with no `/` to permit the ustar prefix split) forces the GNU
        // long-name path. Before the append_data fix, `set_path` failed and
        // the file was silently counted in `skipped_path_too_long`; now it
        // must be archived and round-trip out under its full name.
        let temp = tempfile::TempDir::new().unwrap();
        let long_name = format!("{}.txt", "a".repeat(150));
        let commit_oid = {
            let repo = Repository::init_bare(temp.path()).unwrap();
            let blob = repo.blob(b"long path content\n").unwrap();
            let mut tb = repo.treebuilder(None).unwrap();
            tb.insert(long_name.as_str(), blob, 0o100_644).unwrap();
            let tree = repo.find_tree(tb.write().unwrap()).unwrap();
            let sig = git2::Signature::now("Test", "test@example.com").unwrap();
            repo.commit(Some("HEAD"), &sig, &sig, "long path", &tree, &[])
                .unwrap()
        };
        let repo = open_test_repo(&temp);
        let result = create_tar_from_tree(&repo, commit_oid).unwrap();

        assert_eq!(result.file_count, 1, "the long-path file must be archived");
        assert_eq!(
            result.skipped_path_too_long, 0,
            "long paths must no longer be skipped"
        );

        let names = tar_entry_paths(&result.data);
        assert!(
            names.iter().any(|n| n == &long_name),
            "long path not found in archive entries: {names:?}"
        );
    }

    #[test]
    fn create_tar_reports_file_progress_when_sender_configured() {
        // With a ProgressSender configured, archived files emit FileProcessing
        // updates (rate-limited, so the first always fires). Without a sender
        // these `if let Some(...)` branches never run.
        let (temp, commit_oid) = build_test_repo();
        let repo = open_test_repo(&temp);
        let (sender, receiver) = crate::mcp::progress::ProgressSender::new("t".to_string());
        let opts = TarOptions {
            progress: Some(sender),
            ..Default::default()
        };
        let result = create_tar_from_tree_with_options(&repo, commit_oid, Some(opts)).unwrap();
        assert!(result.file_count >= 1);

        // With a sender configured, create_tar's file loop emits at least one
        // progress update (FileProcessing is the only kind it sends here).
        // Counting via try_iter avoids a matches! arm that would never be taken.
        let update_count = receiver.try_iter().count();
        assert!(
            update_count >= 1,
            "expected at least one progress update with a sender configured"
        );
    }

    #[test]
    fn create_tar_skips_submodules_when_depth_zero() {
        // include_submodules = true but submodule_depth = Some(0) hits the
        // depth==0 early-skip, so no fetch is attempted (no network) and no
        // submodules are included.
        let (temp, commit_oid) = build_test_repo();
        let repo = open_test_repo(&temp);
        let opts = TarOptions {
            include_submodules: Some(true),
            submodule_depth: Some(0),
            ..Default::default()
        };
        let result = create_tar_from_tree_with_options(&repo, commit_oid, Some(opts)).unwrap();
        assert_eq!(result.submodules_included, 0);
        assert_eq!(result.submodules_failed, 0);
    }

    #[test]
    fn create_tar_keeps_lfs_pointer_when_resolve_lfs_but_no_repo_url() {
        // resolve_lfs = true but repo_url = None: the "no repo_url" warn arm
        // runs, the LFS client stays None, and an LFS pointer blob is archived
        // verbatim (no network; lfs_resolved stays 0).
        let temp = tempfile::TempDir::new().unwrap();
        let pointer = b"version https://git-lfs.github.com/spec/v1\n\
                        oid sha256:1111111111111111111111111111111111111111111111111111111111111111\n\
                        size 12\n";
        let commit_oid = {
            let repo = Repository::init_bare(temp.path()).unwrap();
            let blob = repo.blob(pointer).unwrap();
            let mut tb = repo.treebuilder(None).unwrap();
            tb.insert("big.bin", blob, 0o100_644).unwrap();
            let tree = repo.find_tree(tb.write().unwrap()).unwrap();
            let sig = git2::Signature::now("Test", "test@example.com").unwrap();
            repo.commit(Some("HEAD"), &sig, &sig, "lfs pointer", &tree, &[])
                .unwrap()
        };
        let repo = open_test_repo(&temp);
        let opts = TarOptions {
            resolve_lfs: Some(true),
            repo_url: None,
            ..Default::default()
        };
        let result = create_tar_from_tree_with_options(&repo, commit_oid, Some(opts)).unwrap();
        assert_eq!(result.lfs_resolved, 0);
        assert_eq!(result.lfs_failed, 0);
        assert_eq!(result.file_count, 1, "pointer file is archived verbatim");
    }

    #[test]
    fn create_tar_counts_lfs_failure_when_server_unreachable() {
        // resolve_lfs = true with a valid-but-unreachable repo_url: the LFS
        // client IS created, the pointer is parsed and a fetch is attempted,
        // which fails fast (connection refused on 127.0.0.1:1). The failure is
        // counted in `lfs_failed` and the pointer is archived verbatim.
        // `retry_max_attempts: 0` makes the fetch a single attempt (no backoff).
        let temp = tempfile::TempDir::new().unwrap();
        let pointer = b"version https://git-lfs.github.com/spec/v1\n\
                        oid sha256:1111111111111111111111111111111111111111111111111111111111111111\n\
                        size 12\n";
        let commit_oid = {
            let repo = Repository::init_bare(temp.path()).unwrap();
            let blob = repo.blob(pointer).unwrap();
            let mut tb = repo.treebuilder(None).unwrap();
            tb.insert("big.bin", blob, 0o100_644).unwrap();
            let tree = repo.find_tree(tb.write().unwrap()).unwrap();
            let sig = git2::Signature::now("Test", "test@example.com").unwrap();
            repo.commit(Some("HEAD"), &sig, &sig, "lfs pointer", &tree, &[])
                .unwrap()
        };
        let repo = open_test_repo(&temp);
        // A progress sender so the pre-fetch LFS progress report is exercised
        // too; the receiver is kept alive so sends don't fail.
        let (sender, _receiver) = crate::mcp::progress::ProgressSender::new("t".to_string());
        let opts = TarOptions {
            resolve_lfs: Some(true),
            repo_url: Some("https://127.0.0.1:1/repo.git".to_string()),
            lfs_config: Some(crate::config::LfsConfig {
                retry_max_attempts: 0,
                ..Default::default()
            }),
            progress: Some(sender),
            ..Default::default()
        };
        let result = create_tar_from_tree_with_options(&repo, commit_oid, Some(opts)).unwrap();
        assert_eq!(result.lfs_resolved, 0);
        assert_eq!(result.lfs_failed, 1);
        assert_eq!(
            result.file_count, 1,
            "pointer file is archived verbatim on fetch failure"
        );
    }

    #[test]
    fn create_tar_handles_resolve_lfs_with_unsupported_repo_url() {
        // resolve_lfs = true with a repo_url whose scheme derive_lfs_url
        // rejects (ftp://): LfsClient::new errors, the "failed to create LFS
        // client" warn arm runs, the client stays None, and archiving still
        // proceeds for the normal files.
        let (temp, commit_oid) = build_test_repo();
        let repo = open_test_repo(&temp);
        let opts = TarOptions {
            resolve_lfs: Some(true),
            repo_url: Some("ftp://example.com/repo.git".to_string()),
            ..Default::default()
        };
        let result = create_tar_from_tree_with_options(&repo, commit_oid, Some(opts)).unwrap();
        assert_eq!(result.lfs_resolved, 0);
        assert!(result.file_count >= 2);
    }

    #[test]
    fn create_tar_with_lfs_client_archives_non_pointer_files_verbatim() {
        // resolve_lfs = true with a valid https repo_url: the LFS client IS
        // created (the `Ok(client) => Some(client)` arm), but none of the
        // blobs are LFS pointers, so `is_lfs_pointer` is false for each and the
        // Batch API is never called — the files are archived as-is with no
        // network access. Covers the client-created arm and the
        // Some(client)-but-not-a-pointer resolution branch.
        let (temp, commit_oid) = build_test_repo();
        let repo = open_test_repo(&temp);
        let opts = TarOptions {
            resolve_lfs: Some(true),
            repo_url: Some("https://github.com/owner/repo.git".to_string()),
            ..Default::default()
        };
        let result = create_tar_from_tree_with_options(&repo, commit_oid, Some(opts)).unwrap();
        assert_eq!(result.lfs_resolved, 0, "no pointers, so nothing resolved");
        assert_eq!(result.lfs_failed, 0);
        assert!(result.file_count >= 2, "non-pointer files archived as-is");
    }

    #[test]
    fn create_tar_keeps_unparseable_lfs_pointer_verbatim() {
        // A blob whose first line is exactly the v1 version line (so
        // `is_lfs_pointer` is true) but which lacks the `oid`/`size` fields (so
        // `parse_lfs_pointer` returns None). With an LFS client created (valid
        // https repo_url), the Batch API is never called — the malformed pointer
        // is archived verbatim. Exercises the is-pointer-but-unparseable arm.
        let temp = tempfile::TempDir::new().unwrap();
        let commit_oid = {
            let repo = Repository::init_bare(temp.path()).unwrap();
            let blob = repo
                .blob(b"version https://git-lfs.github.com/spec/v1\n")
                .unwrap();
            let mut tb = repo.treebuilder(None).unwrap();
            tb.insert("not-a-real-pointer.bin", blob, 0o100_644)
                .unwrap();
            let tree = repo.find_tree(tb.write().unwrap()).unwrap();
            let sig = git2::Signature::now("Test", "test@example.com").unwrap();
            repo.commit(Some("HEAD"), &sig, &sig, "bad pointer", &tree, &[])
                .unwrap()
        };
        let repo = open_test_repo(&temp);
        let opts = TarOptions {
            resolve_lfs: Some(true),
            repo_url: Some("https://github.com/owner/repo.git".to_string()),
            ..Default::default()
        };
        let result = create_tar_from_tree_with_options(&repo, commit_oid, Some(opts)).unwrap();
        assert_eq!(result.lfs_resolved, 0);
        assert_eq!(
            result.lfs_failed, 0,
            "unparseable pointer is not a fetch failure"
        );
        assert_eq!(result.file_count, 1, "the file is archived verbatim");
    }

    #[test]
    fn create_tar_skips_blob_with_missing_object() {
        // Hand-write a tree with a blob entry pointing at a missing (all-zero)
        // OID, bypassing treebuilder's existence validation. The walk visits the
        // entry (a top-level blob needs no subtree descent), but `find_blob`
        // then fails — exercising the Err arm that logs and skips the blob while
        // the archive still finishes.
        let temp = tempfile::TempDir::new().unwrap();
        let commit_oid = {
            let repo = Repository::init_bare(temp.path()).unwrap();
            let odb = repo.odb().unwrap();
            let mut tree_bytes = Vec::new();
            tree_bytes.extend_from_slice(b"100644 missing.txt\0");
            tree_bytes.extend_from_slice(&[0u8; 20]);
            let tree_oid = odb.write(git2::ObjectType::Tree, &tree_bytes).unwrap();
            let tree = repo.find_tree(tree_oid).unwrap();
            let sig = git2::Signature::now("Test", "test@example.com").unwrap();
            repo.commit(Some("HEAD"), &sig, &sig, "missing blob", &tree, &[])
                .unwrap()
        };
        let repo = open_test_repo(&temp);
        let result = create_tar_from_tree(&repo, commit_oid).unwrap();
        assert_eq!(result.file_count, 0, "the unreadable blob is skipped");
        assert!(!result.data.is_empty(), "the archive still finishes");
    }

    #[test]
    fn tar_mode_for_filemode_maps_correctly() {
        // Regular files keep their permission bits; a symlink (0o120000) has
        // none and must fall back to a readable 0o644 rather than 0o000.
        assert_eq!(tar_mode_for_filemode(0o100_644), 0o644);
        assert_eq!(tar_mode_for_filemode(0o100_755), 0o755);
        assert_eq!(tar_mode_for_filemode(0o120_000), 0o644);
    }

    #[test]
    fn append_blob_to_tar_appends_normal_file() {
        let mut buf = Vec::new();
        let mut skipped = 0usize;
        {
            let encoder = GzEncoder::new(&mut buf, Compression::fast());
            let mut tar_builder = tar::Builder::new(encoder);
            assert!(append_blob_to_tar(
                &mut tar_builder,
                "dir/f.txt",
                b"hi",
                0o100_644,
                &mut skipped
            ));
            tar_builder.finish().unwrap();
        }
        assert_eq!(skipped, 0);
        assert!(tar_entry_paths(&buf).iter().any(|p| p == "dir/f.txt"));
    }

    #[test]
    fn append_blob_to_tar_records_skip_when_write_fails() {
        // A writer that fails every write drives append_data's error path
        // portably — no platform-specific path encoding needed.
        struct FailingWriter;
        impl std::io::Write for FailingWriter {
            fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("boom"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut tar_builder = tar::Builder::new(FailingWriter);
        let mut skipped = 0usize;
        let appended =
            append_blob_to_tar(&mut tar_builder, "x.txt", b"data", 0o100_644, &mut skipped);
        assert!(
            !appended,
            "append must fail when the underlying writer errors"
        );
        assert_eq!(skipped, 1, "a failed append is counted as skipped");

        // The Write contract requires flush; exercise it for completeness.
        assert!(std::io::Write::flush(&mut FailingWriter).is_ok());
    }

    #[test]
    fn create_tar_symlink_entry_gets_readable_mode() {
        // A git symlink is a blob with filemode 0o120000; archived as a regular
        // file, its mode must be normalised so the extracted file is readable
        // (set_mode(0o120000) would otherwise mask to 0o000).
        let temp = tempfile::TempDir::new().unwrap();
        let commit_oid = {
            let repo = Repository::init_bare(temp.path()).unwrap();
            let target = repo.blob(b"src/real.rs").unwrap();
            let mut tb = repo.treebuilder(None).unwrap();
            tb.insert("link.rs", target, 0o120_000).unwrap();
            let tree = repo.find_tree(tb.write().unwrap()).unwrap();
            let sig = git2::Signature::now("Test", "test@example.com").unwrap();
            repo.commit(Some("HEAD"), &sig, &sig, "symlink", &tree, &[])
                .unwrap()
        };
        let repo = open_test_repo(&temp);
        let result = create_tar_from_tree(&repo, commit_oid).unwrap();
        assert_eq!(result.file_count, 1);

        let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(result.data.as_slice()));
        let entry = archive.entries().unwrap().next().unwrap().unwrap();
        let mode = entry.header().mode().unwrap();
        assert_ne!(
            mode & 0o777,
            0,
            "symlink entry must extract with readable permissions"
        );
    }

    /// Build a `FetchedSubmodule` around a locally-created bare repo holding a
    /// single `README.md`, with the given path and recursively-fetched children.
    fn make_fetched_submodule(path: &str, children: Vec<FetchedSubmodule>) -> FetchedSubmodule {
        use crate::git2_ops::clone::FetchResult;
        use crate::git2_ops::submodule::SubmoduleEntry;
        let temp = tempfile::TempDir::new().unwrap();
        let commit_oid = {
            let repo = Repository::init_bare(temp.path()).unwrap();
            let blob = repo.blob(b"sub file\n").unwrap();
            let mut tb = repo.treebuilder(None).unwrap();
            tb.insert("README.md", blob, 0o100_644).unwrap();
            let tree = repo.find_tree(tb.write().unwrap()).unwrap();
            let sig = git2::Signature::now("Test", "test@example.com").unwrap();
            repo.commit(None, &sig, &sig, "sub", &tree, &[]).unwrap()
        };
        let repo = Repository::open_bare(temp.path()).unwrap();
        FetchedSubmodule {
            entry: SubmoduleEntry {
                path: path.to_string(),
                commit: commit_oid,
                url: "https://example.com/sub.git".to_string(),
            },
            fetch_result: FetchResult::from_parts_for_test(
                repo,
                commit_oid,
                "main".to_string(),
                temp,
            ),
            children,
        }
    }

    /// Drive `write_submodules_to_tar` with fresh counters; returns
    /// `(file_count, submodules_included, submodules_failed)`.
    fn run_write_submodules(subs: &[FetchedSubmodule]) -> (usize, usize, usize) {
        let mut buf = Vec::new();
        let mut file_count = 0usize;
        let mut uncompressed_size = 0u64;
        let mut skipped_by_filter = 0usize;
        let mut skipped_binary = 0usize;
        let mut skipped_too_large = 0usize;
        let mut skipped_path_too_long = 0usize;
        let mut submodules_included = 0usize;
        let mut submodules_failed = 0usize;
        {
            let encoder = GzEncoder::new(&mut buf, Compression::fast());
            let mut tar_builder = tar::Builder::new(encoder);
            write_submodules_to_tar(
                subs,
                "",
                None,
                false,
                None,
                &mut tar_builder,
                &mut file_count,
                &mut uncompressed_size,
                &mut skipped_by_filter,
                &mut skipped_binary,
                &mut skipped_too_large,
                &mut skipped_path_too_long,
                &mut submodules_included,
                &mut submodules_failed,
                None,
            );
            tar_builder.finish().unwrap();
        }
        (file_count, submodules_included, submodules_failed)
    }

    #[test]
    fn write_submodules_to_tar_recurses_into_children() {
        // A submodule with a child exercises the recursion arm; files from both
        // levels are archived.
        let child = make_fetched_submodule("inner", Vec::new());
        let parent = make_fetched_submodule("vendor/sub", vec![child]);
        let (file_count, included, failed) = run_write_submodules(std::slice::from_ref(&parent));
        assert_eq!(file_count, 2, "parent + child each contribute one file");
        assert_eq!(included, 2, "both parent and child are included");
        assert_eq!(failed, 0);
    }

    #[test]
    fn write_submodules_to_tar_counts_failure_for_missing_commit() {
        use crate::git2_ops::clone::FetchResult;
        use crate::git2_ops::submodule::SubmoduleEntry;
        // The expected commit is absent from the submodule's repo, so
        // find_commit fails and the submodule is counted as failed.
        let temp = tempfile::TempDir::new().unwrap();
        let repo = Repository::init_bare(temp.path()).unwrap();
        let bogus = Oid::from_str("0000000000000000000000000000000000000001").unwrap();
        let fetched = FetchedSubmodule {
            entry: SubmoduleEntry {
                path: "vendor/sub".to_string(),
                commit: bogus,
                url: "https://example.com/sub.git".to_string(),
            },
            fetch_result: FetchResult::from_parts_for_test(repo, bogus, "main".to_string(), temp),
            children: Vec::new(),
        };
        let (file_count, included, failed) = run_write_submodules(std::slice::from_ref(&fetched));
        assert_eq!(file_count, 0);
        assert_eq!(included, 0);
        assert_eq!(failed, 1, "a missing submodule commit counts as a failure");
    }

    #[test]
    fn write_submodules_to_tar_counts_failure_when_tree_walk_errors() {
        use crate::git2_ops::clone::FetchResult;
        use crate::git2_ops::submodule::SubmoduleEntry;
        // The submodule commit's root tree references a missing subtree object,
        // so walking it fails — exercising the walk_result.is_err() arm.
        let temp = tempfile::TempDir::new().unwrap();
        let commit_oid = {
            let repo = Repository::init_bare(temp.path()).unwrap();
            // Hand-write a tree object that references a non-existent subtree
            // (all-zero OID), bypassing treebuilder's existence validation.
            // Walking it then fails when it descends into "subdir".
            let odb = repo.odb().unwrap();
            let mut tree_bytes = Vec::new();
            tree_bytes.extend_from_slice(b"40000 subdir\0");
            tree_bytes.extend_from_slice(&[0u8; 20]);
            let tree_oid = odb.write(git2::ObjectType::Tree, &tree_bytes).unwrap();
            let tree = repo.find_tree(tree_oid).unwrap();
            let sig = git2::Signature::now("Test", "test@example.com").unwrap();
            repo.commit(None, &sig, &sig, "corrupt", &tree, &[])
                .unwrap()
        };
        let repo = Repository::open_bare(temp.path()).unwrap();
        let fetched = FetchedSubmodule {
            entry: SubmoduleEntry {
                path: "vendor/sub".to_string(),
                commit: commit_oid,
                url: "https://example.com/sub.git".to_string(),
            },
            fetch_result: FetchResult::from_parts_for_test(
                repo,
                commit_oid,
                "main".to_string(),
                temp,
            ),
            children: Vec::new(),
        };
        let (_file_count, included, failed) = run_write_submodules(std::slice::from_ref(&fetched));
        assert_eq!(included, 0);
        assert_eq!(
            failed, 1,
            "a tree-walk failure counts as a submodule failure"
        );
    }

    #[test]
    fn write_submodules_to_tar_applies_filter_size_binary_and_progress() {
        use crate::git2_ops::clone::FetchResult;
        use crate::git2_ops::submodule::SubmoduleEntry;
        // A submodule tree with: a small `.rs` file (kept), a `.md` file
        // (filtered out by the `**/*.rs` sparse pattern), an oversized `.rs`
        // file (skipped by max_file_size), and a binary `.rs` file (skipped by
        // exclude_binary). With a progress sender this drives the filter, size,
        // binary, and progress branches of the submodule walk in one pass.
        let temp = tempfile::TempDir::new().unwrap();
        let commit_oid = {
            let repo = Repository::init_bare(temp.path()).unwrap();
            let small = repo.blob(b"fn a() {}\n").unwrap();
            let md = repo.blob(b"# notes\n").unwrap();
            let big = repo.blob(&[b'x'; 200]).unwrap();
            let binary = repo.blob(b"\x00\x01\x02 binary .rs\x00").unwrap();
            let mut tb = repo.treebuilder(None).unwrap();
            tb.insert("a.rs", small, 0o100_644).unwrap();
            tb.insert("notes.md", md, 0o100_644).unwrap();
            tb.insert("big.rs", big, 0o100_644).unwrap();
            tb.insert("weird.rs", binary, 0o100_644).unwrap();
            let tree = repo.find_tree(tb.write().unwrap()).unwrap();
            let sig = git2::Signature::now("Test", "test@example.com").unwrap();
            repo.commit(None, &sig, &sig, "files", &tree, &[]).unwrap()
        };
        let repo = Repository::open_bare(temp.path()).unwrap();
        let fetched = FetchedSubmodule {
            entry: SubmoduleEntry {
                path: "sub".to_string(),
                commit: commit_oid,
                url: "https://example.com/sub.git".to_string(),
            },
            fetch_result: FetchResult::from_parts_for_test(
                repo,
                commit_oid,
                "main".to_string(),
                temp,
            ),
            children: Vec::new(),
        };

        let filter = SparseFilter::new(&["**/*.rs".to_string()]);
        let (sender, receiver) = crate::mcp::progress::ProgressSender::new("t".to_string());

        let mut buf = Vec::new();
        let mut file_count = 0usize;
        let mut uncompressed_size = 0u64;
        let mut skipped_by_filter = 0usize;
        let mut skipped_binary = 0usize;
        let mut skipped_too_large = 0usize;
        let mut skipped_path_too_long = 0usize;
        let mut submodules_included = 0usize;
        let mut submodules_failed = 0usize;
        {
            let encoder = GzEncoder::new(&mut buf, Compression::fast());
            let mut tar_builder = tar::Builder::new(encoder);
            write_submodules_to_tar(
                std::slice::from_ref(&fetched),
                "",
                Some(&filter),
                true,      // exclude_binary
                Some(100), // max_file_size
                &mut tar_builder,
                &mut file_count,
                &mut uncompressed_size,
                &mut skipped_by_filter,
                &mut skipped_binary,
                &mut skipped_too_large,
                &mut skipped_path_too_long,
                &mut submodules_included,
                &mut submodules_failed,
                Some(&sender),
            );
            tar_builder.finish().unwrap();
        }

        assert_eq!(file_count, 1, "only the small .rs file is archived");
        assert_eq!(skipped_by_filter, 1, "notes.md is filtered out");
        assert_eq!(skipped_too_large, 1, "big.rs is over max_file_size");
        assert_eq!(skipped_binary, 1, "weird.rs is binary");
        assert_eq!(submodules_included, 1);
        assert!(
            receiver.try_iter().count() >= 1,
            "expected a submodule progress update"
        );
    }
}
