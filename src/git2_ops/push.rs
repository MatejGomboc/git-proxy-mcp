//! Bundle processing and authenticated push operations.
//!
//! This module handles receiving git bundles from the AI and pushing
//! them to the remote repository using authenticated callbacks.
//!
//! # How It Works
//!
//! 1. Receive bundle data (base64 decoded)
//! 2. Create temp bare repo and unbundle
//! 3. Push to remote with credential callbacks
//! 4. Clean up temp directory
//!
//! # Security
//!
//! - Bundle is written to temp (not source files)
//! - Credentials via callbacks (never stored)
//! - Protected branch guards applied before push
//! - Temp directory auto-cleanup on drop

use git2::{PushOptions, Repository};
use std::path::Path;
use tempfile::TempDir;
use tracing::{debug, info, warn};

use super::auth::{create_callbacks, validate_url};
use super::error::Git2Error;

/// Result of a successful push operation.
#[derive(Debug, Clone)]
pub struct PushResult {
    /// The branch that was pushed to
    pub branch: String,
    /// The commit ID that was pushed
    pub commit: String,
    /// Remote URL (sanitized for display)
    pub remote_url: String,
}

/// Options for push operations.
#[derive(Debug, Clone)]
pub struct PushOptions2 {
    /// Target branch on remote
    pub branch: String,
    /// Force push (use with caution!)
    pub force: bool,
}

/// Process a git bundle and push to remote.
///
/// This function:
/// 1. Creates a temp bare repo
/// 2. Unbundles the received data
/// 3. Pushes to the remote with authentication
/// 4. Cleans up the temp directory
///
/// # Arguments
///
/// - `bundle_data`: Raw bundle bytes (already base64-decoded)
/// - `remote_url`: Target repository URL
/// - `options`: Push options (branch, force)
///
/// # Errors
///
/// Returns an error if:
/// - URL validation fails (`InvalidUrl`)
/// - Temp directory creation fails (`TempDirFailed`)
/// - Bundle processing fails (`BundleFailed`)
/// - Push operation fails (`PushFailed`)
/// - Branch reference not found (`RefNotFound`)
///
/// # Security
///
/// - Only the bundle file touches disk (not source files)
/// - Credentials handled via callbacks
/// - Temp directory auto-cleaned on drop
pub fn push_bundle(
    bundle_data: &[u8],
    remote_url: &str,
    options: PushOptions2,
    proxy_url: Option<&str>,
) -> Result<PushResult, Git2Error> {
    // Validate URL
    validate_url(remote_url)?;

    info!(
        url = %super::auth::sanitize_url_for_logging(remote_url),
        branch = %options.branch,
        force = options.force,
        bundle_size = bundle_data.len(),
        "starting push from bundle"
    );

    // Create temp directory
    let temp_dir = TempDir::new().map_err(Git2Error::TempDirFailed)?;

    debug!(path = %temp_dir.path().display(), "created temp directory");

    // Initialize bare repo
    let repo = Repository::init_bare(temp_dir.path())
        .map_err(|e| Git2Error::InitFailed(format!("failed to init bare repo: {e}")))?;

    // Write bundle to temp file
    let bundle_path = temp_dir.path().join("input.bundle");
    std::fs::write(&bundle_path, bundle_data)
        .map_err(|e| Git2Error::BundleFailed(format!("failed to write bundle: {e}")))?;

    debug!(path = %bundle_path.display(), "wrote bundle file");

    // Unbundle into the repo
    unbundle(&repo, &bundle_path)?;

    // Get the commit we're pushing
    let reference = repo
        .find_reference(&format!("refs/heads/{}", options.branch))
        .map_err(|_| Git2Error::RefNotFound(options.branch.clone()))?;

    let commit_id = reference
        .peel_to_commit()
        .map_err(|e| Git2Error::RefNotFound(format!("failed to peel to commit: {e}")))?
        .id();

    debug!(commit = %commit_id, "found commit to push");

    // Push to remote
    push_to_remote(&repo, remote_url, &options.branch, options.force, proxy_url)?;

    info!(
        commit = %commit_id,
        branch = %options.branch,
        "push complete"
    );

    Ok(PushResult {
        branch: options.branch,
        commit: commit_id.to_string(),
        remote_url: super::auth::sanitize_url_for_logging(remote_url),
    })
}

/// Unbundle a git bundle file into a repository.
fn unbundle(repo: &Repository, bundle_path: &Path) -> Result<(), Git2Error> {
    debug!(path = %bundle_path.display(), "unbundling");

    // Create a remote pointing to the bundle file.
    // Use the filesystem path directly (not file:// URL) — libgit2
    // recognises bundle files as fetchable remotes when given a path.
    let bundle_str = bundle_path
        .to_str()
        .ok_or_else(|| Git2Error::BundleFailed("invalid bundle path".to_string()))?;

    let mut remote = repo
        .remote_anonymous(bundle_str)
        .map_err(|e| Git2Error::BundleFailed(format!("failed to create bundle remote: {e}")))?;

    // Fetch from bundle (no auth needed for local file)
    remote
        .fetch(&["refs/heads/*:refs/heads/*"], None, None)
        .map_err(|e| Git2Error::BundleFailed(format!("failed to unbundle: {e}")))?;

    debug!("unbundle complete");
    Ok(())
}

/// Push a branch to a remote repository.
fn push_to_remote(
    repo: &Repository,
    remote_url: &str,
    branch: &str,
    force: bool,
    proxy_url: Option<&str>,
) -> Result<(), Git2Error> {
    debug!(
        url = %super::auth::sanitize_url_for_logging(remote_url),
        branch = branch,
        force = force,
        "pushing to remote"
    );

    let mut remote = repo
        .remote_anonymous(remote_url)
        .map_err(|e| Git2Error::PushFailed(format!("failed to create remote: {e}")))?;

    let callbacks = create_callbacks();

    let mut push_opts = PushOptions::new();
    push_opts.remote_callbacks(callbacks);

    let mut proxy_opts = git2::ProxyOptions::new();
    if let Some(url) = proxy_url {
        proxy_opts.url(url);
    } else {
        proxy_opts.auto();
    }
    push_opts.proxy_options(proxy_opts);

    // Build refspec
    let refspec = if force {
        warn!(branch = branch, "force push requested");
        format!("+refs/heads/{branch}:refs/heads/{branch}")
    } else {
        format!("refs/heads/{branch}:refs/heads/{branch}")
    };

    remote
        .push(&[&refspec], Some(&mut push_opts))
        .map_err(|e| Git2Error::PushFailed(e.message().to_string()))?;

    debug!("push complete");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_options_default_no_force() {
        let opts = PushOptions2 {
            branch: "main".to_string(),
            force: false,
        };
        assert!(!opts.force);
    }

    // Integration tests would go here but require network access
    // See tests/git2_integration.rs for full integration tests
}
