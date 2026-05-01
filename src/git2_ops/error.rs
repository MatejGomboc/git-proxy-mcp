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
        // Sanitise the error message to avoid credential leakage
        let message = err.message();

        // Check for authentication-related errors
        if err.class() == git2::ErrorClass::Ssh
            || err.class() == git2::ErrorClass::Http
            || message.contains("auth")
            || message.contains("credential")
            || message.contains("password")
            || message.contains("token")
        {
            return Self::AuthenticationFailed;
        }

        // Generic sanitised error
        Self::Git2(sanitize_error_message(message))
    }
}

/// Sanitise an error message to remove potential credential information.
fn sanitize_error_message(message: &str) -> String {
    // Remove anything that looks like a token or password
    // This is a basic implementation — extend as needed
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
        .collect::<Vec<_>>()
        .join(" ");

    if sanitized.is_empty() {
        "operation failed (details redacted for security)".to_string()
    } else {
        sanitized
    }
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
        match mapped {
            Git2Error::Git2(msg) => assert!(msg.contains("repository not found")),
            other => panic!("expected Git2 variant, got {other:?}"),
        }
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
        match mapped {
            Git2Error::TempDirFailed(inner) => {
                assert_eq!(inner.kind(), io::ErrorKind::PermissionDenied);
            }
            other => panic!("expected TempDirFailed, got {other:?}"),
        }
    }
}
