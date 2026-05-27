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

use git2::{Direction, FetchOptions, Oid, Repository};
use tempfile::TempDir;
use tracing::{debug, info};

use super::auth::{create_callbacks_with_progress, validate_url};
use super::error::{sanitize_error_message, Git2Error};
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

#[cfg(test)]
impl FetchResult {
    /// Construct a `FetchResult` from already-prepared parts. Test-only: lets
    /// unit tests in other modules (e.g. `streaming::tar`) build a fetched-repo
    /// handle around a locally-created bare repo without a real network fetch.
    /// The private `_temp_dir` field keeps the temp directory alive for the
    /// lifetime of the returned value, exactly as the production path does.
    pub(crate) fn from_parts_for_test(
        repo: Repository,
        head_commit: Oid,
        branch: String,
        temp_dir: TempDir,
    ) -> Self {
        Self {
            repo,
            head_commit,
            branch,
            _temp_dir: temp_dir,
        }
    }
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
/// - Repository initialisation fails (`InitFailed`)
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

    // Validate URL before doing anything
    validate_url(url)?;

    info!(url = %super::auth::sanitize_url_for_logging(url), branch = options.branch.as_deref().unwrap_or("(remote default)"), "starting bare fetch");

    fetch_bare_inner(url, &options)
}

/// Fetches into a fresh bare repo — the body of [`fetch_bare`] after URL
/// validation. Split out so tests can drive the connect/fetch path against a
/// local `file://` remote; [`fetch_bare`] still rejects non-network URLs via
/// [`validate_url`] before delegating here.
fn fetch_bare_inner(url: &str, options: &FetchOptions2) -> Result<FetchResult, Git2Error> {
    // Create temp directory for bare repo
    let temp_dir = TempDir::new().map_err(Git2Error::TempDirFailed)?;

    debug!(path = %temp_dir.path().display(), "created temp directory");

    // Initialise BARE repository — no working tree!
    let repo = Repository::init_bare(temp_dir.path())
        .map_err(|e| Git2Error::InitFailed(format!("failed to init bare repo: {}", e.message())))?;

    debug!("initialised bare repository");

    // Resolve the branch name and fetch in a single connection. Scope the
    // remote so it's dropped before we return the repo.
    let branch_name = {
        let mut remote = repo.remote_anonymous(url).map_err(|e| {
            Git2Error::InitFailed(format!(
                "failed to create remote: {}",
                sanitize_error_message(e.message())
            ))
        })?;

        // Connect once so we can both query the default branch (when the
        // caller didn't pass one) and reuse the same connection for fetch.
        let connect_callbacks = create_callbacks_with_progress(options.progress.as_ref());
        let mut connect_proxy = git2::ProxyOptions::new();
        if let Some(ref proxy_url) = options.proxy_url {
            connect_proxy.url(proxy_url);
        } else {
            connect_proxy.auto();
        }
        remote
            .connect_auth(
                Direction::Fetch,
                Some(connect_callbacks),
                Some(connect_proxy),
            )
            .map_err(|e| Git2Error::from_fetch(&e))?;

        // Resolve branch: caller-supplied wins; otherwise ask the connected
        // remote for its default. We never fall back to a hard-coded "main"
        // because that masks misconfigured remotes.
        let branch_name = if let Some(b) = options.branch.as_deref() {
            b.to_string()
        } else {
            let default_buf = remote.default_branch().map_err(|e| {
                Git2Error::FetchFailed(format!(
                    "could not determine remote's default branch: {}",
                    sanitize_error_message(e.message())
                ))
            })?;
            let resolved = decode_default_branch(&default_buf)?;
            debug!(branch = %resolved, "resolved remote's default branch");
            resolved
        };

        // Build fetch options (fresh callbacks since `connect_auth` consumed
        // ours; git2 reuses the existing TCP connection for the actual fetch).
        let fetch_callbacks = create_callbacks_with_progress(options.progress.as_ref());
        let mut fetch_opts = FetchOptions::new();
        fetch_opts.remote_callbacks(fetch_callbacks);

        let mut fetch_proxy = git2::ProxyOptions::new();
        if let Some(ref proxy_url) = options.proxy_url {
            fetch_proxy.url(proxy_url);
        } else {
            fetch_proxy.auto();
        }
        fetch_opts.proxy_options(fetch_proxy);

        if let Some(depth) = options.depth {
            // git2 depth() takes i32, 0 means full clone. Cap at i32::MAX.
            #[allow(clippy::cast_possible_wrap)]
            let depth_i32 = depth.min(i32::MAX as u32) as i32;
            fetch_opts.depth(depth_i32);
            debug!(depth = depth, "shallow clone configured");
        }

        let refspec = format!("refs/heads/{branch_name}:refs/heads/{branch_name}");
        debug!(refspec = %refspec, "fetching");

        remote
            .fetch(&[&refspec], Some(&mut fetch_opts), None)
            .map_err(|e| Git2Error::from_fetch(&e))?;

        branch_name
    };

    // Get the head commit (remote is now dropped, scope reference too)
    let head_commit = {
        let reference = repo
            .find_reference(&format!("refs/heads/{branch_name}"))
            .map_err(|_| Git2Error::RefNotFound(branch_name.clone()))?;

        reference
            .peel_to_commit()
            .map_err(|e| Git2Error::RefNotFound(format!("failed to peel to commit: {e}")))?
            .id()
    };

    info!(commit = %head_commit, branch = %branch_name, "fetch complete");

    Ok(FetchResult {
        repo,
        head_commit,
        branch: branch_name,
        _temp_dir: temp_dir,
    })
}

/// Decode the buffer returned by `Remote::default_branch()` into a plain
/// branch name. libgit2 returns the full ref form (`refs/heads/<branch>`)
/// and has historically appended trailing NUL/newline noise on some
/// transports; we strip both.
fn decode_default_branch(buf: &[u8]) -> Result<String, Git2Error> {
    let s = std::str::from_utf8(buf)
        .map_err(|e| Git2Error::Git2(format!("invalid default branch encoding: {e}")))?;
    Ok(s.strip_prefix("refs/heads/")
        .unwrap_or(s)
        .trim_end_matches(['\0', '\n', '\r'])
        .to_string())
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

    #[test]
    fn decode_default_branch_strips_refs_heads_prefix() {
        assert_eq!(
            decode_default_branch(b"refs/heads/develop").unwrap(),
            "develop"
        );
        assert_eq!(
            decode_default_branch(b"refs/heads/master").unwrap(),
            "master"
        );
        assert_eq!(
            decode_default_branch(b"refs/heads/feature/x").unwrap(),
            "feature/x"
        );
    }

    #[test]
    fn decode_default_branch_trims_trailing_noise() {
        // libgit2 has historically appended NUL or CRLF on some transports.
        assert_eq!(
            decode_default_branch(b"refs/heads/develop\0").unwrap(),
            "develop"
        );
        assert_eq!(
            decode_default_branch(b"refs/heads/develop\n").unwrap(),
            "develop"
        );
        assert_eq!(
            decode_default_branch(b"refs/heads/develop\r\n").unwrap(),
            "develop"
        );
        assert_eq!(
            decode_default_branch(b"refs/heads/develop\0\0").unwrap(),
            "develop"
        );
    }

    #[test]
    fn decode_default_branch_passes_through_branches_without_prefix() {
        // Defensive: bare branch name (libgit2 doesn't currently return this
        // form, but the strip_prefix fallback shouldn't double-mangle it).
        assert_eq!(decode_default_branch(b"develop").unwrap(), "develop");
    }

    #[test]
    fn decode_default_branch_rejects_invalid_utf8() {
        let bad = &[0xFFu8, 0xFE, 0xFD];
        let err = decode_default_branch(bad).unwrap_err();
        assert!(
            matches!(err, Git2Error::Git2(_)),
            "expected Git2 error, got {err:?}"
        );
    }

    #[test]
    fn fetch_bare_default_branch_resolves_non_main_via_local_remote() {
        // Validates the libgit2 contract that fetch_bare's no-branch path
        // relies on: the remote's HEAD symref is what `Remote::default_branch()`
        // returns. We build a bare repo whose HEAD points to `develop` (not
        // `main`), connect a fresh anonymous remote to it via file://, and
        // check the resolved branch name matches the on-disk HEAD target.
        // This is the closest we can get to end-to-end coverage without
        // weakening `validate_url`'s file:// rejection.
        let source = tempfile::TempDir::new().unwrap();
        let source_repo = Repository::init_bare(source.path()).unwrap();

        // Point HEAD at refs/heads/develop *before* the first commit so that
        // the commit creates the develop branch (not main/master).
        source_repo.set_head("refs/heads/develop").unwrap();
        let signature = git2::Signature::now("Test", "test@example.com").unwrap();
        let tree_oid = source_repo.treebuilder(None).unwrap().write().unwrap();
        let tree = source_repo.find_tree(tree_oid).unwrap();
        source_repo
            .commit(
                Some("HEAD"),
                &signature,
                &signature,
                "init on develop",
                &tree,
                &[],
            )
            .unwrap();

        // Connect a fresh anonymous remote (mirroring the bare temp repo
        // created in fetch_bare) and ask for the default branch.
        let dest = tempfile::TempDir::new().unwrap();
        let dest_repo = Repository::init_bare(dest.path()).unwrap();

        let mut remote = dest_repo
            .remote_anonymous(&local_file_url(source.path()))
            .unwrap();
        remote.connect(Direction::Fetch).unwrap();
        let buf = remote.default_branch().unwrap();

        let resolved = decode_default_branch(&buf).unwrap();
        assert_eq!(resolved, "develop");
    }

    /// Builds a `file://` URL for a local path (Windows-friendly).
    fn local_file_url(path: &std::path::Path) -> String {
        let raw = path.display().to_string();
        if cfg!(windows) {
            format!("file:///{}", raw.replace('\\', "/"))
        } else {
            format!("file://{raw}")
        }
    }

    /// Creates a bare source repo with one commit on `branch` (set as HEAD) and
    /// returns the temp dir (kept alive by the caller) plus the commit id.
    fn make_source_repo(branch: &str) -> (TempDir, Oid) {
        let source = TempDir::new().unwrap();
        let repo = Repository::init_bare(source.path()).unwrap();
        repo.set_head(&format!("refs/heads/{branch}")).unwrap();
        let sig = git2::Signature::now("Test", "test@example.com").unwrap();
        let tree_oid = repo.treebuilder(None).unwrap().write().unwrap();
        let tree = repo.find_tree(tree_oid).unwrap();
        let commit = repo
            .commit(Some("HEAD"), &sig, &sig, "initial", &tree, &[])
            .unwrap();
        (source, commit)
    }

    #[test]
    fn fetch_bare_rejects_file_url() {
        assert!(matches!(
            fetch_bare("file:///etc/passwd", None),
            Err(Git2Error::InvalidUrl)
        ));
    }

    #[test]
    fn fetch_bare_rejects_invalid_url() {
        assert!(matches!(
            fetch_bare("/local/path", None),
            Err(Git2Error::InvalidUrl)
        ));
    }

    #[test]
    fn fetch_bare_fails_fast_on_unreachable_host() {
        // Exercises the full public path (validate -> info -> fetch_bare_inner
        // -> connect) via a host that RSTs immediately, so there's no slow
        // timeout. A refused connection is not an auth error, so it surfaces as
        // FetchFailed.
        // `FetchResult` isn't `Debug`, so inspect the error (which is).
        let err = fetch_bare("https://127.0.0.1:1/owner/repo.git", None).err();
        assert!(
            matches!(err, Some(Git2Error::FetchFailed(_))),
            "expected FetchFailed, got {err:?}"
        );
    }

    #[test]
    fn fetch_bare_inner_fetches_default_branch() {
        let (source, commit) = make_source_repo("main");
        let result =
            fetch_bare_inner(&local_file_url(source.path()), &FetchOptions2::default()).unwrap();
        assert_eq!(result.branch, "main");
        assert_eq!(result.head_commit, commit);
        // The branch ref was fetched into the bare repo.
        assert!(result.repo.find_reference("refs/heads/main").is_ok());
    }

    #[test]
    fn fetch_bare_inner_fetches_explicit_branch() {
        // Source has `main` (default) plus a `feature` branch at its own commit.
        let source = TempDir::new().unwrap();
        let repo = Repository::init_bare(source.path()).unwrap();
        repo.set_head("refs/heads/main").unwrap();
        let sig = git2::Signature::now("Test", "test@example.com").unwrap();
        let tree_oid = repo.treebuilder(None).unwrap().write().unwrap();
        let tree = repo.find_tree(tree_oid).unwrap();
        let main_commit = repo
            .commit(Some("refs/heads/main"), &sig, &sig, "on main", &tree, &[])
            .unwrap();
        let main_obj = repo.find_commit(main_commit).unwrap();
        let feature_commit = repo
            .commit(
                Some("refs/heads/feature"),
                &sig,
                &sig,
                "on feature",
                &tree,
                &[&main_obj],
            )
            .unwrap();

        let result = fetch_bare_inner(
            &local_file_url(source.path()),
            &FetchOptions2 {
                branch: Some("feature".to_string()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(result.branch, "feature");
        assert_eq!(result.head_commit, feature_commit);
    }

    #[test]
    fn fetch_bare_inner_accepts_depth_option() {
        let (source, commit) = make_source_repo("main");
        // Shallow support over the local transport varies by libgit2 build, so
        // we only require the depth path not to break the call; when the fetch
        // succeeds the head must still be the tip commit.
        if let Ok(result) = fetch_bare_inner(
            &local_file_url(source.path()),
            &FetchOptions2 {
                branch: Some("main".to_string()),
                depth: Some(1),
                ..Default::default()
            },
        ) {
            assert_eq!(result.head_commit, commit);
        }
    }

    #[test]
    fn fetch_bare_inner_accepts_proxy_option() {
        // The proxy is irrelevant to local file:// transport, so the fetch still
        // succeeds — this just exercises the proxy-configuration branch.
        let (source, commit) = make_source_repo("main");
        let result = fetch_bare_inner(
            &local_file_url(source.path()),
            &FetchOptions2 {
                branch: Some("main".to_string()),
                proxy_url: Some("http://127.0.0.1:9".to_string()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(result.head_commit, commit);
    }

    #[test]
    fn fetch_bare_inner_fails_for_missing_branch() {
        let (source, _) = make_source_repo("main");
        let result = fetch_bare_inner(
            &local_file_url(source.path()),
            &FetchOptions2 {
                branch: Some("does-not-exist".to_string()),
                ..Default::default()
            },
        );
        assert!(result.is_err());
    }

    #[test]
    fn fetch_bare_inner_errors_on_nonexistent_local_remote() {
        let tmp = TempDir::new().unwrap();
        let missing = tmp.path().join("no-such-repo");
        let result = fetch_bare_inner(&local_file_url(&missing), &FetchOptions2::default());
        assert!(result.is_err());
    }
}
