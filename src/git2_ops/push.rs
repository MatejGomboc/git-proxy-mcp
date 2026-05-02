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
use crate::util::sanitize_for_log;

/// Result of a successful push operation.
#[derive(Debug, Clone)]
pub struct PushResult {
    /// The branch that was pushed to
    pub branch: String,
    /// The commit ID that was pushed
    pub commit: String,
    /// Remote URL (sanitised for display)
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
///
/// Uses the `git` CLI because libgit2 does not natively support
/// fetching from bundle files (neither `file://` URLs nor raw paths
/// work reliably across platforms and git/libgit2 versions).
fn unbundle(repo: &Repository, bundle_path: &Path) -> Result<(), Git2Error> {
    debug!(path = %bundle_path.display(), "unbundling");

    let repo_path = repo.path();
    let output = std::process::Command::new("git")
        .args(["fetch", "--no-tags"])
        .arg(bundle_path)
        .arg("refs/heads/*:refs/heads/*")
        .env("GIT_DIR", repo_path)
        .output()
        .map_err(|e| Git2Error::BundleFailed(format!("failed to run git fetch: {e}")))?;

    if !output.status.success() {
        // git's stderr can include ANSI escapes, newlines, and arbitrary
        // bytes from a maliciously-crafted bundle. Sanitise before
        // including in our error message — the error flows into both
        // tracing logs (operator's terminal) and the MCP response
        // (returned to the client / AI). Without this, a bundle that
        // triggers a creative git error could repaint terminal log
        // readers, fake log-line boundaries, or flood the message
        // with megabytes of output.
        let stderr_raw = String::from_utf8_lossy(&output.stderr);
        let stderr_safe = sanitize_for_log(&stderr_raw);
        return Err(Git2Error::BundleFailed(format!(
            "git fetch from bundle failed: {stderr_safe}"
        )));
    }

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

    #[test]
    fn unbundle_sanitises_git_stderr_in_error() {
        // Set up a bare repo + write a malformed bundle. The header is
        // valid (passes `validate_bundle`'s magic check) but the
        // declared ref points to an OID with no pack data behind it,
        // so `git fetch --no-tags <bundle>` fails with multi-line
        // stderr ("fatal: early EOF\nerror: index-pack died" or
        // similar). The newlines are exactly the kind of byte that,
        // unsanitised, would fake a log-line boundary. Verify the
        // returned error has them escaped.
        let temp = TempDir::new().unwrap();
        let repo = Repository::init_bare(temp.path()).unwrap();

        let bundle_path = temp.path().join("bad.bundle");
        std::fs::write(
            &bundle_path,
            b"# v2 git bundle\n\
              0000000000000000000000000000000000000000 refs/heads/main\n\
              \n",
        )
        .unwrap();

        let err = unbundle(&repo, &bundle_path)
            .expect_err("git fetch on a bundle with declared refs but no pack data must fail");
        let msg = err.to_string();

        // `Git2Error::BundleFailed`'s `Display` adds the prefix
        // "bundle processing failed: ", and our `unbundle` adds
        // "git fetch from bundle failed: " inside it.
        assert!(
            msg.contains("git fetch from bundle failed:"),
            "expected our inner prefix, got: {msg:?}"
        );
        // The defining property: any newlines git emitted in stderr
        // were escaped (rendered as `\\n`), so this single error
        // message can't span multiple log lines.
        assert!(
            !msg.contains('\n'),
            "raw newlines from git stderr must be escaped, got: {msg:?}"
        );
        // And no raw ESC either.
        assert!(
            !msg.contains('\x1b'),
            "raw ESC bytes from git stderr must be escaped, got: {msg:?}"
        );
    }
}
