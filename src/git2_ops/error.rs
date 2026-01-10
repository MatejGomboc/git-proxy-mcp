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
    /// Failed to initialize a repository
    #[error("failed to initialize repository: {0}")]
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

    /// Git2 library error (sanitized)
    #[error("git operation failed: {0}")]
    Git2(String),

    /// Bundle processing error
    #[error("bundle processing failed: {0}")]
    BundleFailed(String),
}

impl From<git2::Error> for Git2Error {
    fn from(err: git2::Error) -> Self {
        // Sanitize the error message to avoid credential leakage
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

        // Generic sanitized error
        Self::Git2(sanitize_error_message(message))
    }
}

/// Sanitize an error message to remove potential credential information.
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
}
