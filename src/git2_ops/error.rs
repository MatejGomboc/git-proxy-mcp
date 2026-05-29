//! Error types for git2 operations.
//!
//! These errors are designed to be credential-safe — they never include
//! sensitive information like tokens, passwords, or private keys.

use std::io;
use thiserror::Error;

/// Errors that can occur during git2 operations.
///
/// All error messages are designed to be safe for logging and display
/// without leaking credentials.
#[derive(Error, Debug)]
pub enum Git2Error {
    /// Failed to initialise a repository.
    #[error("failed to initialise repository: {0}")]
    InitFailed(String),

    /// Failed to fetch from remote
    #[error("failed to fetch from remote: {0}")]
    FetchFailed(String),

    /// Failed to push to remote
    #[error("failed to push to remote: {0}")]
    PushFailed(String),

    /// Authentication failed (no credential details exposed)
    #[error("authentication failed — check your SSH agent or credential helper")]
    AuthenticationFailed,

    /// No suitable authentication method available
    #[error("no suitable authentication method available")]
    NoAuthMethod,

    /// Reference not found
    #[error("reference not found: {0}")]
    RefNotFound(String),

    /// Invalid URL
    #[error("invalid repository URL")]
    InvalidUrl,

    /// Temporary directory error
    #[error("failed to create temporary directory: {0}")]
    TempDirFailed(#[from] io::Error),

    /// Git2 library error (sanitised)
    #[error("git operation failed: {0}")]
    Git2(String),

    /// Bundle processing error
    #[error("bundle processing failed: {0}")]
    BundleFailed(String),
}

impl From<git2::Error> for Git2Error {
    fn from(err: git2::Error) -> Self {
        if is_auth_error(&err) {
            Self::AuthenticationFailed
        } else {
            Self::Git2(sanitize_error_message(err.message()))
        }
    }
}

impl Git2Error {
    /// Credential-safe mapping for a git2 error from a *network* operation
    /// (`connect` / `fetch`). Auth-class or auth-keyword errors collapse to
    /// [`Self::AuthenticationFailed`] (no detail leaked); any other message is
    /// run through [`sanitize_error_message`] and wrapped in
    /// [`Self::FetchFailed`].
    ///
    /// Prefer this over `FetchFailed(err.message().to_string())`, which would
    /// let a credential-bearing git2 message (e.g. a URL with embedded
    /// userinfo) reach logs and the client unmodified.
    #[must_use]
    pub(crate) fn from_fetch(err: &git2::Error) -> Self {
        if is_auth_error(err) {
            Self::AuthenticationFailed
        } else {
            Self::FetchFailed(sanitize_error_message(err.message()))
        }
    }

    /// As [`Self::from_fetch`], but non-auth messages are wrapped in
    /// [`Self::PushFailed`].
    #[must_use]
    pub(crate) fn from_push(err: &git2::Error) -> Self {
        if is_auth_error(err) {
            Self::AuthenticationFailed
        } else {
            Self::PushFailed(sanitize_error_message(err.message()))
        }
    }
}

/// Returns `true` if a git2 error is authentication-related, so its message
/// must not be surfaced (it may name the credential helper, an SSH key, or an
/// `Authorization` header).
fn is_auth_error(err: &git2::Error) -> bool {
    let message = err.message();
    matches!(err.class(), git2::ErrorClass::Ssh | git2::ErrorClass::Http)
        || message.contains("auth")
        || message.contains("credential")
        || message.contains("password")
        || message.contains("token")
}

/// Sanitise an error message to remove potential credential information: drops
/// whole lines naming a secret keyword, and redacts the userinfo of any
/// `scheme://user:secret@host` URL the message happened to echo.
pub(crate) fn sanitize_error_message(message: &str) -> String {
    let sanitized = message
        .lines()
        .filter(|line| {
            let lower = line.to_lowercase();
            !lower.contains("password")
                && !lower.contains("token")
                && !lower.contains("secret")
                && !lower.contains("bearer")
                && !lower.contains("authorization")
        })
        .map(redact_url_userinfo)
        .collect::<Vec<_>>()
        .join(" ");

    if sanitized.is_empty() {
        "operation failed (details redacted for security)".to_string()
    } else {
        sanitized
    }
}

/// Replace the userinfo of any `scheme://userinfo@host` substring with `***`,
/// so credentials embedded in a URL that a git2 error echoed never survive.
fn redact_url_userinfo(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut remaining = text;
    while let Some(idx) = remaining.find("://") {
        let scheme_end = idx + 3;
        out.push_str(&remaining[..scheme_end]);
        let after = &remaining[scheme_end..];
        // The authority ends at the first '/', '?', '#', or whitespace.
        let auth_end = after
            .find(|c: char| matches!(c, '/' | '?' | '#') || c.is_whitespace())
            .unwrap_or(after.len());
        let authority = &after[..auth_end];
        if let Some(at) = authority.rfind('@') {
            out.push_str("***");
            out.push_str(&authority[at..]);
        } else {
            out.push_str(authority);
        }
        remaining = &after[auth_end..];
    }
    out.push_str(remaining);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_removes_sensitive_lines() {
        let msg = "Failed to connect\nAuthorization: Bearer abc123\nConnection refused";
        let sanitized = sanitize_error_message(msg);
        assert!(!sanitized.contains("Bearer"));
        assert!(!sanitized.contains("abc123"));
        assert!(sanitized.contains("Failed to connect"));
    }

    #[test]
    fn sanitize_returns_placeholder_when_all_sensitive() {
        let msg = "password: secret123\ntoken: abc";
        let sanitized = sanitize_error_message(msg);
        assert_eq!(
            sanitized,
            "operation failed (details redacted for security)"
        );
    }

    #[test]
    fn sanitize_filters_secret_keyword() {
        let sanitized = sanitize_error_message("ok line\nthe secret is foo");
        assert!(!sanitized.contains("secret"));
        assert!(sanitized.contains("ok line"));
    }

    #[test]
    fn sanitize_is_case_insensitive() {
        let sanitized = sanitize_error_message("PASSWORD reset failed\nfoo");
        assert!(!sanitized.contains("PASSWORD"));
        assert!(sanitized.contains("foo"));
    }

    #[test]
    fn from_git2_ssh_error_maps_to_auth_failed() {
        // Build a synthetic git2 error in the Ssh class.
        let raw_err = git2::Error::new(git2::ErrorCode::Auth, git2::ErrorClass::Ssh, "boom");
        let mapped = Git2Error::from(raw_err);
        assert!(matches!(mapped, Git2Error::AuthenticationFailed));
    }

    #[test]
    fn from_git2_http_error_maps_to_auth_failed() {
        let raw_err = git2::Error::new(
            git2::ErrorCode::GenericError,
            git2::ErrorClass::Http,
            "fail",
        );
        let mapped = Git2Error::from(raw_err);
        assert!(matches!(mapped, Git2Error::AuthenticationFailed));
    }

    #[test]
    fn from_git2_message_contains_auth_keyword() {
        // Message-based detection (e.g., for net-class errors mentioning auth).
        let raw_err = git2::Error::new(
            git2::ErrorCode::GenericError,
            git2::ErrorClass::Net,
            "auth required",
        );
        let mapped = Git2Error::from(raw_err);
        assert!(matches!(mapped, Git2Error::AuthenticationFailed));
    }

    #[test]
    fn from_git2_message_contains_credential_keyword() {
        let raw_err = git2::Error::new(
            git2::ErrorCode::GenericError,
            git2::ErrorClass::Net,
            "no credential helper available",
        );
        assert!(matches!(
            Git2Error::from(raw_err),
            Git2Error::AuthenticationFailed
        ));
    }

    #[test]
    fn from_git2_message_contains_password_keyword() {
        let raw_err = git2::Error::new(
            git2::ErrorCode::GenericError,
            git2::ErrorClass::Net,
            "password expired",
        );
        assert!(matches!(
            Git2Error::from(raw_err),
            Git2Error::AuthenticationFailed
        ));
    }

    #[test]
    fn from_git2_message_contains_token_keyword() {
        let raw_err = git2::Error::new(
            git2::ErrorCode::GenericError,
            git2::ErrorClass::Net,
            "token expired",
        );
        assert!(matches!(
            Git2Error::from(raw_err),
            Git2Error::AuthenticationFailed
        ));
    }

    #[test]
    fn from_git2_generic_error_is_sanitised() {
        let raw_err = git2::Error::new(
            git2::ErrorCode::NotFound,
            git2::ErrorClass::Repository,
            "repository not found",
        );
        let mapped = Git2Error::from(raw_err);
        assert!(
            matches!(&mapped, Git2Error::Git2(msg) if msg.contains("repository not found")),
            "expected Git2 variant, got {mapped:?}"
        );
    }

    #[test]
    fn error_display_messages_match_thiserror() {
        assert_eq!(
            Git2Error::InitFailed("disk full".into()).to_string(),
            "failed to initialise repository: disk full",
        );
        assert_eq!(
            Git2Error::FetchFailed("timeout".into()).to_string(),
            "failed to fetch from remote: timeout",
        );
        assert_eq!(
            Git2Error::PushFailed("rejected".into()).to_string(),
            "failed to push to remote: rejected",
        );
        assert_eq!(
            Git2Error::AuthenticationFailed.to_string(),
            "authentication failed — check your SSH agent or credential helper",
        );
        assert_eq!(
            Git2Error::NoAuthMethod.to_string(),
            "no suitable authentication method available",
        );
        assert_eq!(
            Git2Error::RefNotFound("main".into()).to_string(),
            "reference not found: main",
        );
        assert_eq!(Git2Error::InvalidUrl.to_string(), "invalid repository URL");
        assert_eq!(
            Git2Error::Git2("boom".into()).to_string(),
            "git operation failed: boom",
        );
        assert_eq!(
            Git2Error::BundleFailed("malformed".into()).to_string(),
            "bundle processing failed: malformed",
        );
    }

    #[test]
    fn from_io_error_maps_to_temp_dir_failed() {
        let io_err = io::Error::new(io::ErrorKind::PermissionDenied, "denied");
        let mapped = Git2Error::from(io_err);
        assert!(
            matches!(&mapped, Git2Error::TempDirFailed(inner) if inner.kind() == io::ErrorKind::PermissionDenied),
            "expected TempDirFailed(PermissionDenied), got {mapped:?}"
        );
    }

    #[test]
    fn redact_url_userinfo_strips_credentials() {
        let redacted =
            redact_url_userinfo("unable to access 'https://user:ghp_secret@github.com/o/r.git/'");
        assert!(!redacted.contains("ghp_secret"));
        assert!(!redacted.contains("user:"));
        assert!(redacted.contains("***@github.com"));
        assert!(redacted.contains("github.com/o/r.git"));
    }

    #[test]
    fn redact_url_userinfo_leaves_clean_url_untouched() {
        let text = "failed to connect to https://github.com/o/r.git";
        assert_eq!(redact_url_userinfo(text), text);
    }

    #[test]
    fn redact_url_userinfo_handles_text_without_url() {
        assert_eq!(redact_url_userinfo("object not found"), "object not found");
    }

    #[test]
    fn redact_url_userinfo_redacts_ssh_scheme_userinfo() {
        let redacted = redact_url_userinfo("ssh://git:tok@example.com:22/repo");
        assert!(!redacted.contains("tok"));
        assert!(redacted.contains("***@example.com:22/repo"));
    }

    #[test]
    fn redact_url_userinfo_handles_multiple_urls() {
        let redacted = redact_url_userinfo("from https://a:b@h1/x to https://c:d@h2/y");
        assert!(!redacted.contains("a:b"));
        assert!(!redacted.contains("c:d"));
        assert_eq!(redacted.matches("***@").count(), 2);
    }

    #[test]
    fn sanitize_error_message_redacts_embedded_url_credentials() {
        // The keyword filter would NOT catch a token embedded in a URL; the
        // userinfo redaction does.
        let sanitized =
            sanitize_error_message("unable to access https://u:ghp_x@github.com/o/r.git");
        assert!(!sanitized.contains("ghp_x"));
        assert!(sanitized.contains("***@github.com"));
    }

    #[test]
    fn from_fetch_auth_class_collapses_to_authentication_failed() {
        let err = git2::Error::new(git2::ErrorCode::Auth, git2::ErrorClass::Http, "401");
        assert!(matches!(
            Git2Error::from_fetch(&err),
            Git2Error::AuthenticationFailed
        ));
    }

    #[test]
    fn from_fetch_non_auth_is_sanitised_fetch_failed() {
        let err = git2::Error::new(
            git2::ErrorCode::GenericError,
            git2::ErrorClass::Net,
            "connection refused",
        );
        let mapped = Git2Error::from_fetch(&err);
        assert!(
            matches!(&mapped, Git2Error::FetchFailed(msg) if msg.contains("connection refused")),
            "expected FetchFailed, got {mapped:?}"
        );
    }

    #[test]
    fn from_fetch_redacts_url_credentials_in_message() {
        let err = git2::Error::new(
            git2::ErrorCode::GenericError,
            git2::ErrorClass::Net,
            "failed to resolve https://u:ghp_x@github.com/o/r.git",
        );
        let mapped = Git2Error::from_fetch(&err);
        assert!(
            matches!(&mapped, Git2Error::FetchFailed(msg) if !msg.contains("ghp_x") && msg.contains("***@github.com")),
            "expected redacted FetchFailed, got {mapped:?}"
        );
    }

    #[test]
    fn from_push_non_auth_is_push_failed() {
        let err = git2::Error::new(
            git2::ErrorCode::GenericError,
            git2::ErrorClass::Net,
            "remote rejected",
        );
        let mapped = Git2Error::from_push(&err);
        assert!(
            matches!(&mapped, Git2Error::PushFailed(msg) if msg.contains("remote rejected")),
            "expected PushFailed, got {mapped:?}"
        );
    }

    #[test]
    fn from_push_auth_collapses_to_authentication_failed() {
        let err = git2::Error::new(git2::ErrorCode::Auth, git2::ErrorClass::Ssh, "auth");
        assert!(matches!(
            Git2Error::from_push(&err),
            Git2Error::AuthenticationFailed
        ));
    }
}
