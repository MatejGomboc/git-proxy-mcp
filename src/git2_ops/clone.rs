//! Bare repository fetch operations.
//!
//! This module handles fetching from remote repositories into bare repos.
//! Key design principle: **NO working tree** — source files are never
//! written to disk.
//!
//! # How It Works
//!
//! 1. Create a bare repository in a temp directory
//! 2. Fetch the requested branch/ref using authenticated callbacks
//! 3. Return the repository handle for streaming operations
//! 4. Temp directory is cleaned up when `FetchResult` is dropped
//!
//! # Security
//!
//! - Source files are never checked out (bare repo)
//! - Only git objects (compressed, not plaintext) touch the disk
//! - Temp directory is automatically cleaned on drop

use git2::{FetchOptions, Oid, Repository};
use tempfile::TempDir;
use tracing::{debug, info};

use super::auth::{create_callbacks_with_progress, validate_url};
use super::error::Git2Error;
use crate::mcp::ProgressSender;

/// Result of a successful fetch operation.
///
/// The `_temp_dir` field keeps the temp directory alive. When this struct
/// is dropped, the temp directory and all its contents are deleted.
pub struct FetchResult {
    /// The bare repository containing fetched objects
    pub repo: Repository,
    /// The commit ID at HEAD of the fetched branch
    pub head_commit: Oid,
    /// The branch name that was fetched
    pub branch: String,
    /// Temp directory handle — dropping this cleans up the repo
    _temp_dir: TempDir,
}

/// Options for fetch operations.
#[derive(Debug, Clone, Default)]
pub struct FetchOptions2 {
    /// Branch to fetch (defaults to the remote's default branch)
    pub branch: Option<String>,
    /// Shallow clone depth (None = full history)
    pub depth: Option<u32>,
    /// Optional progress sender for real-time updates
    pub progress: Option<ProgressSender>,
    /// Optional proxy URL (None = auto-detect from environment)
    pub proxy_url: Option<String>,
}

/// Fetch a repository without creating a working tree.
///
/// This creates a bare repository and fetches the specified branch.
/// Source files are never written to disk — only git objects.
///
/// # Arguments
///
/// - `url`: Repository URL (https:// or git@)
/// - `options`: Fetch options (branch, depth)
///
/// # Returns
///
/// A `FetchResult` containing the repository and metadata. The temp
/// directory is cleaned up when this result is dropped.
///
/// # Errors
///
/// Returns an error if:
/// - URL validation fails (`InvalidUrl`)
/// - Temp directory creation fails (`TempDirFailed`)
/// - Repository initialization fails (`InitFailed`)
/// - Fetch operation fails (`FetchFailed`)
/// - Branch reference not found (`RefNotFound`)
///
/// # Security
///
/// - Uses credential callbacks (no credentials stored)
/// - Bare repository (no source files on disk)
/// - Temp directory auto-cleanup on drop
///
/// # Example
///
/// ```ignore
/// let result = fetch_bare("https://github.com/owner/repo.git", None)?;
/// // Use result.repo to access git objects
/// // Temp directory cleaned up when result is dropped
/// ```
pub fn fetch_bare(url: &str, options: Option<FetchOptions2>) -> Result<FetchResult, Git2Error> {
    let options = options.unwrap_or_default();
    let branch_name = options.branch.as_deref().unwrap_or("main");

    // Validate URL before doing anything
    validate_url(url)?;

    info!(
        url = %super::auth::sanitize_url_for_logging(url),
        branch = branch_name,
        "starting bare fetch"
    );

    // Create temp directory for bare repo
    let temp_dir = TempDir::new().map_err(Git2Error::TempDirFailed)?;

    debug!(path = %temp_dir.path().display(), "created temp directory");

    // Initialize BARE repository — no working tree!
    let repo = Repository::init_bare(temp_dir.path())
        .map_err(|e| Git2Error::InitFailed(format!("failed to init bare repo: {}", e.message())))?;

    debug!("initialized bare repository");

    // Fetch the specific branch
    let refspec = format!("refs/heads/{branch_name}:refs/heads/{branch_name}");

    // Scope the remote to ensure it's dropped before we return the repo
    {
        let mut remote = repo
            .remote_anonymous(url)
            .map_err(|e| Git2Error::InitFailed(format!("failed to create remote: {e}")))?;

        let callbacks = create_callbacks_with_progress(options.progress.as_ref());

        let mut fetch_opts = FetchOptions::new();
        fetch_opts.remote_callbacks(callbacks);

        let mut proxy_opts = git2::ProxyOptions::new();
        if let Some(ref proxy_url) = options.proxy_url {
            proxy_opts.url(proxy_url);
        } else {
            proxy_opts.auto();
        }
        fetch_opts.proxy_options(proxy_opts);

        // Configure shallow clone if depth is specified
        if let Some(depth) = options.depth {
            // git2 depth() takes i32, 0 means full clone
            // Cap at i32::MAX and convert safely
            #[allow(clippy::cast_possible_wrap)]
            let depth_i32 = depth.min(i32::MAX as u32) as i32;
            fetch_opts.depth(depth_i32);
            debug!(depth = depth, "shallow clone configured");
        }

        debug!(refspec = %refspec, "fetching");

        remote
            .fetch(&[&refspec], Some(&mut fetch_opts), None)
            .map_err(|e| Git2Error::FetchFailed(e.message().to_string()))?;
    }

    // Get the head commit (remote is now dropped, scope reference too)
    let head_commit = {
        let reference = repo
            .find_reference(&format!("refs/heads/{branch_name}"))
            .map_err(|_| Git2Error::RefNotFound(branch_name.to_string()))?;

        reference
            .peel_to_commit()
            .map_err(|e| Git2Error::RefNotFound(format!("failed to peel to commit: {e}")))?
            .id()
    };

    info!(
        commit = %head_commit,
        branch = branch_name,
        "fetch complete"
    );

    Ok(FetchResult {
        repo,
        head_commit,
        branch: branch_name.to_string(),
        _temp_dir: temp_dir,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fetch_options_default() {
        let opts = FetchOptions2::default();
        assert!(opts.branch.is_none());
        assert!(opts.depth.is_none());
        assert!(opts.proxy_url.is_none());
    }

    // Integration tests would go here but require network access
    // See tests/git2_integration.rs for full integration tests
}
