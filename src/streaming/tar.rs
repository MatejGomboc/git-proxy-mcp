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

/// Options for tar creation.
#[derive(Debug, Clone, Default)]
pub struct TarOptions {
    /// Sparse checkout patterns — only include files matching these glob patterns.
    /// If empty or None, all files are included.
    pub sparse_patterns: Option<Vec<String>>,
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

/// Result of creating a tar archive.
#[derive(Debug)]
pub struct TarResult {
    /// The compressed tar.gz data
    pub data: Vec<u8>,
    /// Number of files included
    pub file_count: usize,
    /// Total uncompressed size
    pub uncompressed_size: u64,
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
pub fn create_tar_from_tree_with_options(
    repo: &Repository,
    commit_id: Oid,
    options: Option<TarOptions>,
) -> Result<TarResult, Git2Error> {
    let options = options.unwrap_or_default();

    debug!(commit = %commit_id, sparse = ?options.sparse_patterns, "creating tar from tree");

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

    let mut archive_buffer = Vec::new();
    let mut file_count = 0usize;
    let mut uncompressed_size = 0u64;
    let mut skipped_by_filter = 0usize;

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
                        let content = blob.content();

                        trace!(path = %path, size = content.len(), "adding file to tar");

                        // Create tar header
                        let mut header = tar::Header::new_gnu();
                        if header.set_path(&path).is_err() {
                            // Path too long, try to handle gracefully
                            debug!(path = %path, "path too long for tar, skipping");
                            return TreeWalkResult::Ok;
                        }
                        header.set_size(content.len() as u64);
                        // filemode() returns i32, but negative modes are invalid
                        #[allow(clippy::cast_sign_loss)]
                        header.set_mode(entry.filemode() as u32);
                        header.set_cksum();

                        // Append to tar
                        if tar_builder.append(&header, content).is_err() {
                            debug!(path = %path, "failed to append to tar, skipping");
                            return TreeWalkResult::Ok;
                        }

                        file_count += 1;
                        uncompressed_size += content.len() as u64;
                    }
                    Err(e) => {
                        debug!(path = %path, error = %e, "failed to read blob, skipping");
                    }
                }
            }

            TreeWalkResult::Ok
        })
        .map_err(|e| Git2Error::Git2(format!("tree walk failed: {e}")))?;

        // Finish the tar archive
        tar_builder
            .finish()
            .map_err(|e| Git2Error::Git2(format!("failed to finish tar: {e}")))?;
    }

    debug!(
        file_count = file_count,
        skipped_by_filter = skipped_by_filter,
        uncompressed_size = uncompressed_size,
        compressed_size = archive_buffer.len(),
        "tar creation complete"
    );

    Ok(TarResult {
        data: archive_buffer,
        file_count,
        uncompressed_size,
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
    }

    // Integration tests that require a git repo are in tests/streaming_tests.rs
}
