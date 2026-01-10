//! Create tar.gz archives from git trees in memory.
//!
//! This module walks a git tree and creates a compressed tar archive
//! containing all files, without ever writing source files to disk.
//!
//! # How It Works
//!
//! 1. Walk the git tree recursively
//! 2. For each blob (file), read content from git object database
//! 3. Append to tar archive (in memory)
//! 4. Compress with gzip
//! 5. Return raw bytes (for base64 encoding)
//!
//! # Memory Usage
//!
//! Tier 1: O(repository size) — entire archive buffered in memory.
//! This is acceptable for small-to-medium repos.

use flate2::write::GzEncoder;
use flate2::Compression;
use git2::{ObjectType, Oid, Repository, TreeWalkMode, TreeWalkResult};
use tracing::{debug, trace};

use crate::git2_ops::error::Git2Error;

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
    debug!(commit = %commit_id, "creating tar from tree");

    let commit = repo
        .find_commit(commit_id)
        .map_err(|e| Git2Error::Git2(format!("failed to find commit: {e}")))?;

    let tree = commit
        .tree()
        .map_err(|e| Git2Error::Git2(format!("failed to get tree: {e}")))?;

    let mut archive_buffer = Vec::new();
    let mut file_count = 0usize;
    let mut uncompressed_size = 0u64;

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

    // Integration tests that require a git repo are in tests/streaming_tests.rs
}
