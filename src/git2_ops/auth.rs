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
//! - Private keys never leave the SSH agent

use git2::{Cred, CredentialType, RemoteCallbacks};
use tracing::{debug, warn};

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
pub fn create_callbacks_with_progress<'a>(progress: Option<&ProgressSender>) -> RemoteCallbacks<'a> {
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
    debug!(
        url = %sanitize_url_for_logging(url),
        username = ?username_from_url,
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

    Err(git2::Error::from_str("no suitable credential method available"))
}

/// Sanitize a URL for logging by removing any embedded credentials.
///
/// URLs like `https://user:token@github.com/...` become `https://***@github.com/...`
#[must_use]
pub fn sanitize_url_for_logging(url: &str) -> String {
    // Check for credentials in URL (https://user:pass@host/...)
    if let Some(at_pos) = url.find('@') {
        if let Some(scheme_end) = url.find("://") {
            let scheme = &url[..scheme_end + 3];
            let after_at = &url[at_pos + 1..];
            return format!("{scheme}***@{after_at}");
        }
    }
    url.to_string()
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
}
