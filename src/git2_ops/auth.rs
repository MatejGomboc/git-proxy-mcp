//! Credential callbacks for git2 operations.
//!
//! This module provides secure credential handling using git2's callback system.
//! Credentials are retrieved from the user's existing configuration:
//!
//! - **SSH keys**: Via ssh-agent (private key never leaves the agent)
//! - **HTTPS tokens**: Via system credential helpers (macOS Keychain,
//!   Windows Credential Manager, libsecret, etc.)
//!
//! # Supported Providers
//!
//! Works with any Git hosting provider that supports standard protocols:
//!
//! - **GitHub** (github.com)
//! - **GitLab** (gitlab.com and self-hosted)
//! - **Bitbucket** (bitbucket.org)
//! - **Azure DevOps** (dev.azure.com)
//! - **Any self-hosted Git server** (Gitea, Gogs, etc.)
//!
//! # Security Guarantees
//!
//! - Credentials are NEVER logged
//! - Credentials are NEVER stored in memory longer than needed
//! - Credentials are NEVER included in error messages
//! - Credentials are NEVER sent to AI (they stay on user's PC)
//! - Private keys never leave the SSH agent
//!
//! # Credential Retrieval for LFS
//!
//! The [`get_credentials_for_url`] function retrieves credentials from the OS
//! credential store using the git credential helper protocol. This allows LFS
//! operations to authenticate without storing credentials in the MCP server.

use git2::{Cred, CredentialType, RemoteCallbacks};
use std::io::Write;
use std::process::{Command, Stdio};
use tracing::{debug, trace, warn};
use url::Url;

use super::error::Git2Error;
use crate::mcp::ProgressSender;

/// Creates git2 remote callbacks with credential handling.
///
/// The callbacks support:
/// - SSH agent authentication (for git@ URLs)
/// - Credential helper authentication (for https:// URLs)
///
/// # Security
///
/// This function does not store or log any credentials. The credential
/// callbacks retrieve secrets on-demand and they are used only for the
/// duration of the git operation.
#[must_use]
pub fn create_callbacks<'a>() -> RemoteCallbacks<'a> {
    create_callbacks_with_progress(None)
}

/// Creates git2 remote callbacks with credential handling and optional progress reporting.
///
/// Same as `create_callbacks` but accepts an optional progress sender for real-time
/// transfer progress updates during fetch operations.
///
/// # Arguments
///
/// - `progress`: Optional progress sender for reporting transfer progress
///
/// # Progress Updates
///
/// When a progress sender is provided, the callback reports:
/// - Received bytes
/// - Total bytes (if known)
/// - Received objects
/// - Total objects
/// - Indexed objects
#[must_use]
pub fn create_callbacks_with_progress<'a>(
    progress: Option<&ProgressSender>,
) -> RemoteCallbacks<'a> {
    let mut callbacks = RemoteCallbacks::new();

    callbacks.credentials(credentials_callback);

    // Clone the progress sender for the closure
    let progress_sender = progress.cloned();

    // Log transfer progress and optionally send to progress channel
    callbacks.transfer_progress(move |stats| {
        debug!(
            received = stats.received_objects(),
            total = stats.total_objects(),
            bytes = stats.received_bytes(),
            "transfer progress"
        );

        // Send progress update if we have a sender
        if let Some(ref sender) = progress_sender {
            sender.send_transfer(
                stats.received_bytes(),
                0, // git2 doesn't provide total_bytes upfront
                stats.received_objects(),
                stats.total_objects(),
                stats.indexed_objects(),
            );
        }

        true
    });

    callbacks
}

/// Credential callback for git2 operations.
///
/// This callback is invoked by git2 when authentication is required.
/// It attempts authentication methods in order of preference:
///
/// 1. SSH agent (for SSH URLs)
/// 2. Credential helper (for HTTPS URLs)
///
/// # Arguments
///
/// - `url`: The URL being accessed (used for credential lookup)
/// - `username_from_url`: Username extracted from the URL (e.g., "git" from git@github.com)
/// - `allowed_types`: Credential types the server accepts
///
/// # Security
///
/// The `Cred` object returned contains sensitive data but is managed by git2
/// and not exposed to our code. We never log or store the credential.
fn credentials_callback(
    url: &str,
    username_from_url: Option<&str>,
    allowed_types: CredentialType,
) -> Result<Cred, git2::Error> {
    // Log only whether a username was present, never its value. For HTTPS a
    // token is commonly supplied as the username (e.g. `https://<PAT>@host`),
    // so `username_from_url` can itself be a secret — logging it verbatim would
    // leak the credential at debug level. The URL is sanitised the same way.
    debug!(
        url = %sanitize_url_for_logging(url),
        username_present = username_from_url.is_some(),
        allowed_types = ?allowed_types,
        "credential callback invoked"
    );

    // Try SSH agent first (most secure — key never leaves agent)
    if allowed_types.contains(CredentialType::SSH_KEY) {
        if let Some(username) = username_from_url {
            debug!("attempting SSH agent authentication");
            match Cred::ssh_key_from_agent(username) {
                Ok(cred) => {
                    debug!("SSH agent authentication successful");
                    return Ok(cred);
                }
                Err(e) => {
                    debug!(error = %e, "SSH agent authentication failed, trying other methods");
                }
            }
        }
    }

    // Try credential helper (for HTTPS)
    if allowed_types.contains(CredentialType::USER_PASS_PLAINTEXT) {
        debug!("attempting credential helper authentication");
        match git2::Config::open_default() {
            Ok(config) => match Cred::credential_helper(&config, url, username_from_url) {
                Ok(cred) => {
                    debug!("credential helper authentication successful");
                    return Ok(cred);
                }
                Err(e) => {
                    debug!(error = %e, "credential helper authentication failed");
                }
            },
            Err(e) => {
                warn!(error = %e, "failed to open git config");
            }
        }
    }

    // Try default credentials (username without password for some SSH configs)
    if allowed_types.contains(CredentialType::DEFAULT) {
        debug!("attempting default authentication");
        if let Ok(cred) = Cred::default() {
            return Ok(cred);
        }
    }

    Err(git2::Error::from_str(
        "no suitable credential method available",
    ))
}

/// Sanitise a URL for logging by removing any embedded credentials.
///
/// URLs like `https://user:token@github.com/...` become `https://***@github.com/...`.
/// Per RFC 3986, the userinfo `@` separator can only appear in the authority
/// component (between `://` and the first `/` `?` or `#`); any `@` later in
/// the URL is part of the path, query, or fragment and is left untouched.
/// SSH-style URLs without `://` (e.g. `git@host:path`) are also returned
/// unchanged — SSH authentication uses keys via `ssh-agent` rather than
/// inline URL credentials, so there's nothing to strip.
#[must_use]
pub fn sanitize_url_for_logging(url: &str) -> String {
    let Some(scheme_end) = url.find("://") else {
        // No scheme separator: SSH-style or otherwise non-URL input.
        // We never inject `@`-style userinfo into these, so leave alone.
        return url.to_string();
    };

    let after_scheme_idx = scheme_end + 3;
    let after_scheme = &url[after_scheme_idx..];

    // The authority ends at the first path/query/fragment delimiter.
    let authority_end = after_scheme
        .find(['/', '?', '#'])
        .unwrap_or(after_scheme.len());
    let authority = &after_scheme[..authority_end];

    // `rfind('@')` so any (would-be percent-encoded) `@` earlier in userinfo
    // doesn't fool us into stripping less than the full userinfo.
    let Some(at_in_authority) = authority.rfind('@') else {
        return url.to_string();
    };

    let scheme_part = &url[..after_scheme_idx];
    let after_at_idx = after_scheme_idx + at_in_authority + 1;
    let host_and_rest = &url[after_at_idx..];
    format!("{scheme_part}***@{host_and_rest}")
}

/// Validate that a URL is acceptable for operations.
///
/// This performs basic validation without making network requests.
///
/// # Errors
///
/// Returns `Git2Error::InvalidUrl` if the URL:
/// - Has no scheme (no `://` or `git@` prefix)
/// - Uses dangerous schemes like `file://` or `ext::`
pub fn validate_url(url: &str) -> Result<(), Git2Error> {
    // Must have a scheme
    if !url.contains("://") && !url.starts_with("git@") {
        return Err(Git2Error::InvalidUrl);
    }

    // Block obviously dangerous schemes
    let lower = url.to_lowercase();
    if lower.starts_with("file://") || lower.starts_with("ext::") {
        return Err(Git2Error::InvalidUrl);
    }

    Ok(())
}

/// Retrieve credentials for a URL from the OS credential store.
///
/// This uses the git credential helper protocol to retrieve credentials from
/// the user's configured credential store (macOS Keychain, Windows Credential
/// Manager, git-credential-manager, etc.).
///
/// # Security
///
/// - Credentials are retrieved on-demand from OS-level credential stores
/// - Credentials NEVER leave the user's PC
/// - Credentials are NEVER logged or stored persistently by this MCP server
/// - This function returns the credentials in memory only for immediate use
///
/// # Arguments
///
/// * `url` - The URL to get credentials for (e.g., `https://github.com/owner/repo.git`)
///
/// # Returns
///
/// `Some((username, password))` if credentials were found, `None` otherwise.
///
/// # Example
///
/// ```ignore
/// let creds = get_credentials_for_url("https://github.com/owner/repo.git");
/// if let Some((user, pass)) = creds {
///     // Use credentials for LFS authentication
/// }
/// ```
#[must_use]
pub fn get_credentials_for_url(url: &str) -> Option<(String, String)> {
    // Parse the URL to extract protocol and host
    let parsed = parse_url_for_credentials(url)?;

    trace!(
        protocol = %parsed.0,
        host = %parsed.1,
        "retrieving credentials from git credential helper"
    );

    // Build the input for git credential fill
    let input = format!(
        "protocol={}\nhost={}\n\n",
        parsed.0, // protocol (https)
        parsed.1  // host (github.com)
    );

    // Run `git credential fill`. Each failure mode is debug-traced so a
    // user investigating "no credentials" in LFS logs can tell whether
    // the cause was missing git, a stdin write error, an exit-non-zero
    // helper, or a malformed response. `GIT_TERMINAL_PROMPT=0` forbids
    // git from prompting on the terminal — an MCP server has no UI to
    // surface a prompt to anyway, and an unattended prompt would just
    // hang the subprocess until timeout.
    let mut child = match Command::new("git")
        .args(["credential", "fill"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(e) => {
            debug!(error = %e, "failed to spawn `git credential fill` (is `git` on PATH?)");
            return None;
        }
    };

    if let Some(stdin) = child.stdin.as_mut() {
        if let Err(e) = stdin.write_all(input.as_bytes()) {
            debug!(error = %e, "failed to write to `git credential fill` stdin");
            return None;
        }
    }

    let output = match child.wait_with_output() {
        Ok(output) => output,
        Err(e) => {
            debug!(error = %e, "failed to wait for `git credential fill`");
            return None;
        }
    };

    if !output.status.success() {
        debug!(exit = ?output.status.code(), "`git credential fill` exited non-zero (no credential helper configured?)");
        return None;
    }

    parse_credential_fill_stdout(&output.stdout)
}

/// Parse the stdout of a successful `git credential fill` invocation
/// into `(username, password)`.
///
/// Returns `None` if:
/// - `stdout` is not valid UTF-8.
/// - The output is missing `username=` or `password=` lines.
///
/// Extracted from `get_credentials_for_url` so the post-subprocess
/// parsing logic is unit-testable without spawning a real `git` process.
fn parse_credential_fill_stdout(stdout: &[u8]) -> Option<(String, String)> {
    let stdout_str = match std::str::from_utf8(stdout) {
        Ok(s) => s,
        Err(e) => {
            debug!(error = %e, "`git credential fill` stdout was not valid UTF-8");
            return None;
        }
    };
    let creds = parse_credential_output(stdout_str);
    if creds.is_none() {
        debug!("`git credential fill` returned without `username` and `password` fields");
    }
    creds
}

/// Parse a URL into (protocol, host) for credential lookup.
///
/// SSH URLs are recognised only in the canonical `git@host:path` form;
/// alternative SSH users (`gitea@`, `gerrit@`, etc.) and `ssh://` URLs
/// are handled by the second branch via `Url::parse`.
fn parse_url_for_credentials(url: &str) -> Option<(String, String)> {
    // Handle SSH URLs: git@github.com:owner/repo.git
    if url.starts_with("git@") {
        // Extract host from git@host:path
        let without_prefix = url.strip_prefix("git@")?;
        // Trim whitespace so a malformed-but-recoverable host like
        // `git@ github.com :path` resolves to `github.com` rather than
        // ` github.com` (which would silently miss credentials).
        let host = without_prefix.split(':').next()?.trim();
        // Defensive: empty or whitespace-only host (e.g. `git@`,
        // `git@:path`, `git@   :path`) is not a valid lookup key —
        // `git credential fill` with `host=` (or whitespace) would
        // either fail or, worse, return credentials for a different
        // configured host by accident.
        if host.is_empty() {
            return None;
        }
        return Some(("https".to_string(), host.to_string()));
    }

    // Handle standard URLs
    let parsed = Url::parse(url).ok()?;
    let protocol = parsed.scheme().to_string();
    let host = parsed.host_str()?.to_string();

    Some((protocol, host))
}

/// Parse git credential output into (username, password).
fn parse_credential_output(output: &str) -> Option<(String, String)> {
    let mut username = None;
    let mut password = None;

    for line in output.lines() {
        if let Some((key, value)) = line.split_once('=') {
            match key {
                "username" => username = Some(value.to_string()),
                "password" => password = Some(value.to_string()),
                _ => {}
            }
        }
    }

    // Both username and password are required
    Some((username?, password?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_url_removes_credentials() {
        let url = "https://user:secret_token@github.com/owner/repo.git";
        let sanitized = sanitize_url_for_logging(url);
        assert!(!sanitized.contains("secret_token"));
        assert!(!sanitized.contains("user:"));
        assert!(sanitized.contains("***@"));
        assert!(sanitized.contains("github.com/owner/repo.git"));
    }

    #[test]
    fn sanitize_url_preserves_clean_urls() {
        let url = "https://github.com/owner/repo.git";
        let sanitized = sanitize_url_for_logging(url);
        assert_eq!(sanitized, url);
    }

    #[test]
    fn sanitize_url_handles_ssh() {
        let url = "git@github.com:owner/repo.git";
        let sanitized = sanitize_url_for_logging(url);
        // SSH URLs with @ but no :// should be preserved
        assert_eq!(sanitized, url);
    }

    #[test]
    fn validate_url_accepts_https() {
        assert!(validate_url("https://github.com/owner/repo.git").is_ok());
    }

    #[test]
    fn validate_url_accepts_ssh() {
        assert!(validate_url("git@github.com:owner/repo.git").is_ok());
    }

    #[test]
    fn validate_url_rejects_file() {
        assert!(validate_url("file:///etc/passwd").is_err());
    }

    #[test]
    fn validate_url_rejects_no_scheme() {
        assert!(validate_url("/some/local/path").is_err());
    }

    // Multi-provider support tests
    #[test]
    fn validate_url_accepts_gitlab_https() {
        assert!(validate_url("https://gitlab.com/owner/repo.git").is_ok());
    }

    #[test]
    fn validate_url_accepts_gitlab_ssh() {
        assert!(validate_url("git@gitlab.com:owner/repo.git").is_ok());
    }

    #[test]
    fn validate_url_accepts_bitbucket_https() {
        assert!(validate_url("https://bitbucket.org/owner/repo.git").is_ok());
    }

    #[test]
    fn validate_url_accepts_bitbucket_ssh() {
        assert!(validate_url("git@bitbucket.org:owner/repo.git").is_ok());
    }

    #[test]
    fn validate_url_accepts_self_hosted_gitlab() {
        assert!(validate_url("https://gitlab.example.com/group/project.git").is_ok());
        assert!(validate_url("git@gitlab.example.com:group/project.git").is_ok());
    }

    #[test]
    fn validate_url_accepts_azure_devops() {
        assert!(validate_url("https://dev.azure.com/org/project/_git/repo").is_ok());
        assert!(validate_url("git@ssh.dev.azure.com:v3/org/project/repo").is_ok());
    }

    #[test]
    fn sanitize_url_handles_gitlab_credentials() {
        let url = "https://oauth2:token@gitlab.com/owner/repo.git";
        let sanitized = sanitize_url_for_logging(url);
        assert!(!sanitized.contains("token"));
        assert!(sanitized.contains("***@gitlab.com"));
    }

    #[test]
    fn sanitize_url_handles_bitbucket_credentials() {
        let url = "https://username:app_password@bitbucket.org/owner/repo.git";
        let sanitized = sanitize_url_for_logging(url);
        assert!(!sanitized.contains("app_password"));
        assert!(sanitized.contains("***@bitbucket.org"));
    }

    // Credential helper URL parsing tests
    #[test]
    fn parse_url_for_credentials_https() {
        let result = parse_url_for_credentials("https://github.com/owner/repo.git");
        assert_eq!(
            result,
            Some(("https".to_string(), "github.com".to_string()))
        );
    }

    #[test]
    fn parse_url_for_credentials_http() {
        let result = parse_url_for_credentials("http://gitlab.example.com/owner/repo.git");
        assert_eq!(
            result,
            Some(("http".to_string(), "gitlab.example.com".to_string()))
        );
    }

    #[test]
    fn parse_url_for_credentials_ssh() {
        let result = parse_url_for_credentials("git@github.com:owner/repo.git");
        // SSH URLs use https protocol for credential lookup
        assert_eq!(
            result,
            Some(("https".to_string(), "github.com".to_string()))
        );
    }

    #[test]
    fn parse_url_for_credentials_gitlab_ssh() {
        let result = parse_url_for_credentials("git@gitlab.com:owner/repo.git");
        assert_eq!(
            result,
            Some(("https".to_string(), "gitlab.com".to_string()))
        );
    }

    #[test]
    fn parse_url_for_credentials_invalid() {
        let result = parse_url_for_credentials("not-a-url");
        assert!(result.is_none());
    }

    // Credential output parsing tests
    #[test]
    fn parse_credential_output_valid() {
        let output = "protocol=https\nhost=github.com\nusername=myuser\npassword=mytoken\n";
        let result = parse_credential_output(output);
        assert_eq!(result, Some(("myuser".to_string(), "mytoken".to_string())));
    }

    #[test]
    fn parse_credential_output_missing_username() {
        let output = "protocol=https\nhost=github.com\npassword=mytoken\n";
        let result = parse_credential_output(output);
        assert!(result.is_none());
    }

    #[test]
    fn parse_credential_output_missing_password() {
        let output = "protocol=https\nhost=github.com\nusername=myuser\n";
        let result = parse_credential_output(output);
        assert!(result.is_none());
    }

    #[test]
    fn parse_credential_output_empty() {
        let result = parse_credential_output("");
        assert!(result.is_none());
    }

    #[test]
    fn validate_url_rejects_empty_string() {
        assert!(validate_url("").is_err());
    }

    #[test]
    fn validate_url_rejects_ext_protocol() {
        assert!(validate_url("ext::ssh user@host").is_err());
    }

    #[test]
    fn validate_url_rejects_uppercase_file_scheme() {
        assert!(validate_url("FILE:///etc/passwd").is_err());
    }

    #[test]
    fn validate_url_rejects_uppercase_ext() {
        assert!(validate_url("EXT::ssh whatever").is_err());
    }

    #[test]
    fn validate_url_accepts_http() {
        assert!(validate_url("http://example.com/repo.git").is_ok());
    }

    #[test]
    fn sanitize_url_handles_https_no_credentials() {
        let url = "https://github.com";
        let sanitized = sanitize_url_for_logging(url);
        assert_eq!(sanitized, url);
    }

    #[test]
    fn sanitize_url_with_only_username_and_at() {
        let url = "https://username@github.com/owner/repo.git";
        let sanitized = sanitize_url_for_logging(url);
        // No password but has @ — still sanitised
        assert!(sanitized.contains("***@github.com"));
        assert!(!sanitized.contains("username"));
    }

    #[test]
    fn sanitize_url_with_at_but_no_scheme_returns_unchanged() {
        let url = "user@host:path";
        let sanitized = sanitize_url_for_logging(url);
        // Has @ but no :// — unchanged
        assert_eq!(sanitized, url);
    }

    #[test]
    fn parse_url_for_credentials_with_port() {
        let result = parse_url_for_credentials("https://gitlab.example.com:8443/owner/repo.git");
        assert_eq!(
            result,
            Some(("https".to_string(), "gitlab.example.com".to_string()))
        );
    }

    #[test]
    fn parse_url_for_credentials_ssh_with_no_path() {
        // SSH URL with just user@host (no colon)
        let result = parse_url_for_credentials("git@github.com");
        // Should still parse — host is everything after git@
        // Actually this depends on whether the implementation handles this case
        // If `host:path` split returns whole string when no colon, host = "github.com"
        assert!(result.is_some());
    }

    #[test]
    fn parse_url_for_credentials_url_no_host() {
        // URL with scheme but no host
        let result = parse_url_for_credentials("file:///local");
        // file:// has no host, so host_str() returns None
        assert!(result.is_none());
    }

    #[test]
    fn parse_credential_output_with_extra_fields() {
        let output =
            "protocol=https\nhost=github.com\nusername=u\npassword=p\ncapability[]=authtype\n";
        let result = parse_credential_output(output);
        assert_eq!(result, Some(("u".to_string(), "p".to_string())));
    }

    #[test]
    fn parse_credential_output_handles_lines_without_equals() {
        let output = "protocol=https\nno_equals_here\nusername=u\npassword=p\n";
        let result = parse_credential_output(output);
        assert_eq!(result, Some(("u".to_string(), "p".to_string())));
    }

    #[test]
    fn parse_credential_output_preserves_value_with_equals() {
        // split_once means values with = are preserved (only first = splits)
        let output = "username=u\npassword=p=with=equals\n";
        let result = parse_credential_output(output);
        assert_eq!(result, Some(("u".to_string(), "p=with=equals".to_string())));
    }

    #[test]
    fn create_callbacks_returns_callbacks() {
        // Just exercise the function — we can't easily test the closures
        // without invoking them with real git2 args.
        let _callbacks = create_callbacks();
        // If we got here without panicking, the function works.
    }

    #[test]
    fn create_callbacks_with_progress_returns_callbacks() {
        let (sender, _receiver) = crate::mcp::progress::ProgressSender::new("t".to_string());
        let _callbacks = create_callbacks_with_progress(Some(&sender));
        let _callbacks_no_progress = create_callbacks_with_progress(None);
    }

    #[test]
    fn validate_url_rejects_just_scheme_no_host() {
        // Edge case — has :// but nothing meaningful
        // This actually passes validation since it checks for "://" presence,
        // not validity. Document the current behaviour.
        assert!(validate_url("https://").is_ok());
    }

    #[test]
    fn parse_url_for_credentials_ssh_extracts_correct_host() {
        let result = parse_url_for_credentials("git@bitbucket.org:owner/repo.git");
        assert_eq!(
            result,
            Some(("https".to_string(), "bitbucket.org".to_string()))
        );
    }

    #[test]
    fn parse_url_for_credentials_with_userinfo() {
        let result = parse_url_for_credentials("https://user:pass@github.com/repo.git");
        assert_eq!(
            result,
            Some(("https".to_string(), "github.com".to_string()))
        );
    }

    #[test]
    fn parse_url_for_credentials_rejects_empty_ssh_host() {
        // `git@:path` has an empty host before the colon — sending
        // `host=` to `git credential fill` would either fail or match
        // a default-configured host by accident. Must reject.
        assert!(parse_url_for_credentials("git@:owner/repo.git").is_none());
    }

    #[test]
    fn parse_url_for_credentials_rejects_just_git_at() {
        // `git@` alone (no host, no path) — also empty-host.
        assert!(parse_url_for_credentials("git@").is_none());
    }

    #[test]
    fn parse_url_for_credentials_rejects_whitespace_only_ssh_host() {
        // Whitespace-only hosts must be rejected too — `host.is_empty()`
        // alone wouldn't catch them, so the impl trims first.
        assert!(parse_url_for_credentials("git@   :path").is_none());
        assert!(parse_url_for_credentials("git@\t:path").is_none());
    }

    #[test]
    fn parse_url_for_credentials_trims_whitespace_padded_ssh_host() {
        // A non-empty host with surrounding whitespace gets trimmed
        // rather than passed through as `" github.com"`, which
        // `git credential fill` would silently miss.
        let result = parse_url_for_credentials("git@ github.com :owner/repo.git");
        assert_eq!(
            result,
            Some(("https".to_string(), "github.com".to_string()))
        );
    }

    #[test]
    fn sanitize_url_strips_userinfo_when_fragment_also_has_at() {
        // Combinatorial check: BOTH userinfo `@` AND a later `@` in the
        // fragment must result in only the userinfo being stripped — the
        // fragment must survive intact (this case wasn't covered
        // separately by the path/query combo test).
        let url = "https://user:secret@github.com/owner/repo#section@anchor";
        let sanitized = sanitize_url_for_logging(url);
        assert!(!sanitized.contains("secret"));
        assert!(!sanitized.contains("user:"));
        assert!(sanitized.contains("***@github.com/owner/repo#section@anchor"));
    }

    #[test]
    fn parse_credential_fill_stdout_returns_creds_on_well_formed_output() {
        // Happy-path: the helper parses well-formed `git credential fill`
        // stdout and returns the username/password.
        let stdout = b"protocol=https\nhost=github.com\nusername=alice\npassword=tok\n";
        let result = parse_credential_fill_stdout(stdout);
        assert_eq!(result, Some(("alice".to_string(), "tok".to_string())));
    }

    #[test]
    fn parse_credential_fill_stdout_returns_none_on_missing_fields() {
        // No username/password lines — the helper returns None and
        // (under debug-level tracing) emits a diagnostic.
        let stdout = b"protocol=https\nhost=github.com\n";
        let result = parse_credential_fill_stdout(stdout);
        assert!(result.is_none());
    }

    #[test]
    fn parse_credential_fill_stdout_returns_none_on_invalid_utf8() {
        // Helpers that emit binary garbage (or a wrong-encoding output)
        // hit the UTF-8 error arm — the helper returns None rather than
        // panicking.
        let stdout = &[0xFFu8, 0xFE, 0xFD];
        let result = parse_credential_fill_stdout(stdout);
        assert!(result.is_none());
    }

    #[test]
    fn parse_credential_fill_stdout_returns_none_on_empty_input() {
        let result = parse_credential_fill_stdout(b"");
        assert!(result.is_none());
    }

    #[test]
    fn get_credentials_for_url_returns_none_for_unconfigured_host() {
        // End-to-end test that exercises `get_credentials_for_url`'s
        // subprocess success path: spawn `git`, write to stdin, wait for
        // output, parse the result. We use a host under `.invalid` (an
        // RFC 6761 reserved TLD that is guaranteed never to be configured
        // anywhere) so any installed credential helper will return
        // nothing, and `GIT_TERMINAL_PROMPT=0` (set inside the function)
        // prevents any helper from prompting on a developer machine.
        //
        // The function must return `None` (no credentials available),
        // having gone through the spawn / write / wait / parse pipeline.
        // This pulls coverage of the credential-fill body up from zero
        // — the function's existing rustdoc was not backed by any test.
        let result = get_credentials_for_url("https://nonexistent-coverage-test.invalid/repo.git");
        assert!(
            result.is_none(),
            "no credentials should be configured for an .invalid TLD host"
        );
    }

    #[test]
    fn credentials_callback_falls_through_when_no_supported_method_allowed() {
        // The callback supports SSH_KEY, USER_PASS_PLAINTEXT, and DEFAULT
        // credential types; if `allowed_types` contains none of these
        // (e.g. `CredentialType::empty()` or only USERNAME / SSH_INTERACTIVE
        // / SSH_MEMORY / SSH_CUSTOM), every branch is skipped and the
        // function returns the fallback `git2::Error`. This is the only
        // branch testable without a live remote — the SSH-agent and
        // credential-helper branches depend on system state and would be
        // flaky on CI runners without configured credentials.
        let result = credentials_callback(
            "https://github.com/owner/repo.git",
            None,
            CredentialType::empty(),
        );
        // `Cred` doesn't implement `Debug`, so we can't use `.unwrap_err()`
        // here — go through `Option<E>` instead.
        let err = result
            .err()
            .expect("expected error when no credential type is allowed");
        let msg = err.message();
        assert!(
            msg.contains("no suitable credential method available"),
            "expected fallback message, got: {msg}"
        );
    }

    #[test]
    fn credentials_callback_returns_default_cred_for_default_type() {
        // `Cred::default()` never fails, so when only the DEFAULT credential
        // type is allowed the callback returns it directly — exercising the
        // DEFAULT branch deterministically, without touching ssh-agent or any
        // credential helper.
        let result = credentials_callback(
            "https://example.com/repo.git",
            None,
            CredentialType::DEFAULT,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn credentials_callback_attempts_ssh_agent_branch() {
        // Exercises the SSH-agent branch. Whether it returns Ok (an agent with
        // a usable key is present) or Err (none) depends on system state, so —
        // unlike the existing fallback test — this only asserts the branch runs
        // without panicking, never the outcome (keeping it non-flaky on CI).
        let _ = credentials_callback(
            "git@github.com:owner/repo.git",
            Some("git"),
            CredentialType::SSH_KEY,
        );
    }

    #[test]
    fn sanitize_url_does_not_mangle_at_in_query() {
        // Regression: previously, the function found the FIRST `@` anywhere
        // in the URL and treated everything between `://` and that `@` as
        // userinfo. So a URL with `@` in a query string was mangled — the
        // host and path were silently dropped from the log output. RFC 3986
        // restricts the userinfo `@` separator to the authority component.
        let url = "https://github.com/owner/repo?email=foo@bar.com";
        let sanitized = sanitize_url_for_logging(url);
        assert_eq!(
            sanitized, url,
            "URL with `@` only in query string must round-trip unchanged"
        );
    }

    #[test]
    fn sanitize_url_does_not_mangle_at_in_path() {
        // Same regression for `@` in path component.
        let url = "https://github.com/owner@user/repo.git";
        let sanitized = sanitize_url_for_logging(url);
        assert_eq!(sanitized, url);
    }

    #[test]
    fn sanitize_url_does_not_mangle_at_in_fragment() {
        let url = "https://github.com/owner/repo#sec@something";
        let sanitized = sanitize_url_for_logging(url);
        assert_eq!(sanitized, url);
    }

    #[test]
    fn sanitize_url_strips_userinfo_when_path_also_has_at() {
        // Both userinfo `@` and a later `@` in path/query: only userinfo
        // should be replaced, the rest of the URL must survive intact.
        let url = "https://user:secret@github.com/owner/repo?email=foo@bar.com";
        let sanitized = sanitize_url_for_logging(url);
        assert!(!sanitized.contains("secret"));
        assert!(!sanitized.contains("user:"));
        assert!(sanitized.contains("***@github.com/owner/repo?email=foo@bar.com"));
    }

    #[test]
    fn sanitize_url_handles_authority_with_port_and_userinfo() {
        let url = "https://user:tok@gitlab.example.com:8443/group/project.git";
        let sanitized = sanitize_url_for_logging(url);
        assert!(!sanitized.contains("tok"));
        assert!(sanitized.contains("***@gitlab.example.com:8443/group/project.git"));
    }

    #[test]
    fn sanitize_url_strips_token_used_as_username() {
        // A GitHub PAT supplied as the username (no password) — a common
        // pattern (`https://<PAT>@github.com/...`). The whole userinfo, token
        // included, must be replaced with `***`.
        let url = "https://ghp_REALLOOKINGSECRET1234567890@github.com/owner/repo.git";
        let sanitized = sanitize_url_for_logging(url);
        assert!(!sanitized.contains("ghp_REALLOOKINGSECRET1234567890"));
        assert!(!sanitized.contains("REALLOOKINGSECRET"));
        assert_eq!(sanitized, "https://***@github.com/owner/repo.git");
    }

    /// Minimal [`std::io::Write`] that appends to a shared buffer so a test can
    /// capture `tracing` output and assert credentials never appear in it.
    #[derive(Clone)]
    struct CaptureWriter(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for CaptureWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .expect("capture buffer poisoned")
                .extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn credentials_callback_never_logs_a_token_username() {
        // Defence-in-depth regression: with a DEBUG subscriber active, the
        // credential callback's entry log must NOT contain a token supplied as
        // the URL username. `CredentialType::empty()` is used so no real
        // credential method is attempted (no ssh-agent / credential-helper /
        // network), isolating the test to the logging path.
        let token = "ghp_REALLOOKINGSECRET1234567890";
        let url = format!("https://{token}@github.com/owner/repo.git");

        let buffer = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = buffer.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .with_writer(move || CaptureWriter(sink.clone()))
            .finish();

        let result = tracing::subscriber::with_default(subscriber, || {
            credentials_callback(&url, Some(token), CredentialType::empty())
        });
        // No credential method was allowed, so the callback reports failure.
        assert!(result.is_err());

        let logged = String::from_utf8(buffer.lock().unwrap().clone()).unwrap();
        assert!(
            logged.contains("credential callback invoked"),
            "entry log should have been emitted; got: {logged}"
        );
        assert!(
            !logged.contains(token) && !logged.contains("REALLOOKINGSECRET"),
            "token leaked into the credential-callback log: {logged}"
        );
        // The sanitised host is still logged for diagnostics.
        assert!(logged.contains("github.com"));

        // The fmt layer is line-buffered and may never call the writer's
        // `flush`, so exercise it directly to confirm it is a clean no-op.
        std::io::Write::flush(&mut CaptureWriter(buffer)).unwrap();
    }
}
