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

    // Create temp directory.
    // Defensive `?` against system errors — the failure arm is not
    // unit-testable without env-var manipulation that breaks parallel
    // test isolation. The OK path is exercised by every push test.
    let temp_dir = TempDir::new().map_err(Git2Error::TempDirFailed)?;

    debug!(path = %temp_dir.path().display(), "created temp directory");

    // Initialise bare repo.
    // Defensive `?` against system errors — `init_bare` only fails
    // when the path is invalid, but we just got it from a fresh
    // `TempDir`. Not externally triggerable; OK path is exercised.
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

    // Defensive `?` against corrupted refs — `peel_to_commit` only
    // fails when the ref points to a non-peelable object (e.g. a
    // raw tree), which `git bundle create` never produces. Not
    // externally triggerable; OK path is exercised.
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

    /// Returns `true` if the `git` CLI is on PATH and usable.
    ///
    /// Several tests need `git bundle create` (to produce a real
    /// bundle) or rely on `unbundle`, which itself shells out to
    /// `git fetch`. CI installs git in every job, so on the project's
    /// own runners this always returns `true`. The helper exists so
    /// that running `cargo test` in a sandbox without git produces
    /// a graceful skip rather than a panic deep inside the helper.
    fn git_is_available() -> bool {
        std::process::Command::new("git")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    }

    /// Skips the current test (with an `eprintln!` trace) if `git`
    /// isn't on PATH. Macro form so the early-return happens in the
    /// caller, not inside the helper. Uses the test's thread name
    /// (which Cargo sets to the full test path) for the trace.
    macro_rules! require_git {
        () => {
            if !git_is_available() {
                let name = std::thread::current()
                    .name()
                    .unwrap_or("<unnamed>")
                    .to_string();
                eprintln!("skipping {name}: git CLI not on PATH");
                return;
            }
        };
    }

    #[test]
    fn push_options_default_no_force() {
        let opts = PushOptions2 {
            branch: "main".to_string(),
            force: false,
        };
        assert!(!opts.force);
    }

    #[test]
    fn push_options_force_flag_round_trips() {
        // Document the Clone derive on PushOptions2 — exercises both
        // fields plus the impl.
        let opts = PushOptions2 {
            branch: "release".to_string(),
            force: true,
        };
        let cloned = opts.clone();
        assert!(cloned.force);
        assert_eq!(cloned.branch, "release");
        // Original is still usable post-clone.
        assert_eq!(opts.branch, "release");
    }

    #[test]
    fn push_result_fields_are_accessible() {
        // PushResult is the public success type; document its shape and
        // exercise the Clone derive (keeps the original alive).
        let result = PushResult {
            branch: "main".to_string(),
            commit: "deadbeef".to_string(),
            remote_url: "https://github.com/o/r.git".to_string(),
        };
        let cloned = result.clone();
        assert_eq!(cloned.branch, "main");
        assert_eq!(cloned.commit, "deadbeef");
        assert!(cloned.remote_url.contains("github.com"));
        assert_eq!(result.commit, "deadbeef");
    }

    /// Build a bare repo with a single commit on `branch`, produce a git
    /// bundle for it via the `git bundle create` CLI, and return the
    /// bundle bytes plus the temp dir (kept alive so the bundle file
    /// stays valid across drops if tests want the path).
    ///
    /// Bundles produced this way are real git bundles — `push_bundle`'s
    /// internal `git fetch --no-tags <bundle>` accepts them.
    fn make_bundle_for_branch(branch: &str) -> (TempDir, Vec<u8>) {
        let temp = TempDir::new().unwrap();
        {
            let repo = Repository::init_bare(temp.path()).unwrap();
            let signature = git2::Signature::now("Test", "test@example.com").unwrap();
            let blob = repo.blob(b"hello\n").unwrap();
            let mut tb = repo.treebuilder(None).unwrap();
            tb.insert("hello.txt", blob, 0o100_644).unwrap();
            let tree_oid = tb.write().unwrap();
            let tree = repo.find_tree(tree_oid).unwrap();
            let head_ref = format!("refs/heads/{branch}");
            repo.commit(
                Some(&head_ref),
                &signature,
                &signature,
                "seed commit",
                &tree,
                &[],
            )
            .unwrap();
        }

        let bundle_path = temp.path().join("output.bundle");
        let output = std::process::Command::new("git")
            .args(["bundle", "create"])
            .arg(&bundle_path)
            .arg(branch)
            .env("GIT_DIR", temp.path())
            .output()
            .expect("git bundle create must succeed");
        assert!(
            output.status.success(),
            "git bundle create failed: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );

        let bytes = std::fs::read(&bundle_path).expect("bundle file must exist");
        (temp, bytes)
    }

    #[test]
    fn push_bundle_rejects_invalid_url() {
        // First branch in push_bundle: validate_url() rejects file://
        // (and other unsupported schemes). No temp dir or bundle work
        // happens — the function returns InvalidUrl immediately.
        let opts = PushOptions2 {
            branch: "main".to_string(),
            force: false,
        };
        let result = push_bundle(b"unused", "file:///etc/passwd", opts, None);
        assert!(matches!(result, Err(Git2Error::InvalidUrl)));
    }

    #[test]
    fn push_bundle_rejects_ext_url() {
        // ext:: URLs are also rejected by validate_url. Same fast path.
        let opts = PushOptions2 {
            branch: "main".to_string(),
            force: false,
        };
        let result = push_bundle(b"unused", "ext::malicious", opts, None);
        assert!(matches!(result, Err(Git2Error::InvalidUrl)));
    }

    #[test]
    fn push_bundle_fails_with_malformed_bundle_data() {
        require_git!();
        // URL passes validate_url, temp + init succeed, write succeeds —
        // but the bundle bytes aren't a real git bundle, so unbundle's
        // `git fetch` invocation fails with BundleFailed. Exercises
        // push_bundle lines 84-111 (validate, temp dir, init_bare,
        // write, unbundle call).
        let opts = PushOptions2 {
            branch: "main".to_string(),
            force: false,
        };
        let result = push_bundle(
            b"this is definitely not a git bundle",
            "https://example.invalid/repo.git",
            opts,
            None,
        );
        assert!(
            matches!(result, Err(Git2Error::BundleFailed(_))),
            "got: {result:?}"
        );
    }

    #[test]
    fn push_bundle_returns_ref_not_found_when_branch_missing_after_unbundle() {
        require_git!();
        // Bundle has branch "main", but caller asks for "feature/missing".
        // Unbundle succeeds, find_reference fails — covers lines 114-116
        // (the `?` on find_reference returning RefNotFound).
        let (_keep_alive, bundle_bytes) = make_bundle_for_branch("main");
        let opts = PushOptions2 {
            branch: "feature/missing".to_string(),
            force: false,
        };
        let result = push_bundle(
            &bundle_bytes,
            "https://example.invalid/repo.git",
            opts,
            None,
        );
        match result {
            Err(Git2Error::RefNotFound(name)) => {
                assert_eq!(name, "feature/missing");
            }
            other => panic!("expected RefNotFound, got: {other:?}"),
        }
    }

    #[test]
    fn push_bundle_fails_when_remote_unreachable_after_successful_unbundle() {
        require_git!();
        // Full happy path through unbundle + ref resolution + push call —
        // the actual push fails because 127.0.0.1:1 RSTs immediately.
        // Covers the body of push_bundle (lines 86-128) AND the body of
        // push_to_remote (lines 179-224) up to the push call's error.
        let (_keep_alive, bundle_bytes) = make_bundle_for_branch("main");
        let opts = PushOptions2 {
            branch: "main".to_string(),
            force: false,
        };
        let result = push_bundle(
            &bundle_bytes,
            // 127.0.0.1:1 is the system's TCPMUX port — almost never
            // listening, OS rejects with TCP RST immediately, no slow
            // timeout.
            "http://127.0.0.1:1/repo.git",
            opts,
            None,
        );
        assert!(
            matches!(result, Err(Git2Error::PushFailed(_))),
            "got: {result:?}"
        );
    }

    #[test]
    fn push_bundle_force_path_takes_force_refspec() {
        require_git!();
        // Same as the unreachable test, but with force=true. Covers the
        // `if force { ... } else { ... }` branch in push_to_remote
        // (line 211) which the non-force test above doesn't reach.
        let (_keep_alive, bundle_bytes) = make_bundle_for_branch("main");
        let opts = PushOptions2 {
            branch: "main".to_string(),
            force: true,
        };
        let result = push_bundle(&bundle_bytes, "http://127.0.0.1:1/repo.git", opts, None);
        // Same outcome as non-force (push fails on unreachable host),
        // but the +refs/heads/main:refs/heads/main refspec branch is
        // now exercised.
        assert!(
            matches!(result, Err(Git2Error::PushFailed(_))),
            "got: {result:?}"
        );
    }

    #[test]
    fn push_bundle_passes_proxy_through_to_push_to_remote() {
        require_git!();
        // The proxy_url argument is forwarded into push_to_remote's
        // `git2::ProxyOptions::url()` call, exercising the
        // `if let Some(url) = proxy_url` branch (line 203).
        //
        // We don't try to verify that libgit2 actually CONNECTs through
        // the proxy at the network level — libgit2 chooses between
        // direct and proxy transports based on the remote URL scheme
        // (HTTP vs HTTPS), and a fake-proxy harness needs the full
        // proxy protocol (CONNECT for HTTPS, full-URL GET for HTTP)
        // plus the smart-HTTP framing on top. That's a much larger
        // test fixture for one branch.
        //
        // What this test DOES assert: passing a proxy URL doesn't
        // panic, doesn't break the call path, and produces a graceful
        // PushFailed (not a different error variant) when the upstream
        // is unreachable.
        let (_keep_alive, bundle_bytes) = make_bundle_for_branch("main");
        let opts = PushOptions2 {
            branch: "main".to_string(),
            force: false,
        };
        let result = push_bundle(
            &bundle_bytes,
            "http://127.0.0.1:1/repo.git",
            opts,
            Some("http://127.0.0.1:1"),
        );
        assert!(
            matches!(result, Err(Git2Error::PushFailed(_))),
            "got: {result:?}"
        );
    }

    #[test]
    fn unbundle_succeeds_with_valid_bundle() {
        require_git!();
        // Happy path for unbundle: produce a real bundle, unbundle it
        // into a fresh bare repo, confirm the branch ref now exists.
        // Covers lines 154-157, 174-175 (the success arms).
        let (_keep_alive, bundle_bytes) = make_bundle_for_branch("main");

        let target = TempDir::new().unwrap();
        let target_repo = Repository::init_bare(target.path()).unwrap();
        let bundle_path = target.path().join("input.bundle");
        std::fs::write(&bundle_path, &bundle_bytes).unwrap();

        unbundle(&target_repo, &bundle_path).expect("unbundle of valid bundle must succeed");

        // After unbundle, refs/heads/main should resolve to a commit.
        let reference = target_repo
            .find_reference("refs/heads/main")
            .expect("refs/heads/main must exist after unbundle");
        let commit = reference.peel_to_commit().unwrap();
        // git2 0.21: `Commit::message()` returns `Result` (UTF-8 check).
        assert_eq!(commit.message(), Ok("seed commit"));
    }

    #[test]
    fn unbundle_returns_bundle_failed_when_bundle_path_missing() {
        require_git!();
        // Unbundle's `git fetch --no-tags <bundle_path>` errors out
        // when the bundle file doesn't exist. Tests the "git ran but
        // exited non-zero" path with a path-not-found stderr — same
        // sanitisation logic as the malformed-bundle test, but a
        // distinct trigger condition.
        let temp = TempDir::new().unwrap();
        let repo = Repository::init_bare(temp.path()).unwrap();
        let missing = temp.path().join("does-not-exist.bundle");
        let err = unbundle(&repo, &missing).expect_err("unbundle of nonexistent path must fail");
        let msg = err.to_string();
        assert!(
            msg.contains("git fetch from bundle failed"),
            "expected unbundle's prefix, got: {msg:?}"
        );
        // No raw newlines (sanitisation invariant).
        assert!(!msg.contains('\n'), "got: {msg:?}");
    }

    #[test]
    fn unbundle_sanitises_git_stderr_in_error() {
        require_git!();
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
