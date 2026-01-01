//! Error types for git-proxy-mcp.
//!
//! # Security Note
//!
//! Error messages are carefully crafted to NEVER include credentials.
//! All error variants that could potentially contain sensitive data
//! use generic descriptions instead of including the actual values.

use std::path::PathBuf;

use thiserror::Error;

/// Errors that can occur during credential operations.
///
/// # Security
///
/// These errors intentionally do NOT include credential values in their
/// messages. Even in debug builds, credentials must never appear in logs
/// or error output.
#[derive(Error, Debug)]
pub enum AuthError {
    /// No matching credential found for the given URL.
    ///
    /// The URL is included for debugging, but the actual credentials
    /// that were searched are never exposed.
    #[error("no credential found matching URL: {url}")]
    NoMatchingCredential {
        /// The URL that was being matched.
        url: String,
    },

    /// SSH key file could not be read.
    ///
    /// Only the path is included, never the key contents.
    #[error("failed to read SSH key from path: {path}")]
    SshKeyReadError {
        /// Path to the SSH key file.
        path: PathBuf,
        /// The underlying IO error.
        #[source]
        source: std::io::Error,
    },

    /// SSH key file does not exist.
    #[error("SSH key file not found: {path}")]
    SshKeyNotFound {
        /// Path to the missing SSH key file.
        path: PathBuf,
    },

    /// SSH agent is not available.
    #[error("SSH agent not available or not running")]
    SshAgentUnavailable,

    /// Invalid credential configuration.
    ///
    /// The specific field is named, but its value is never included.
    #[error("invalid credential configuration: {message}")]
    InvalidCredential {
        /// Description of what is invalid (without revealing the value).
        message: String,
    },

    /// URL pattern is malformed.
    #[error("invalid URL pattern '{pattern}': {reason}")]
    InvalidUrlPattern {
        /// The malformed pattern.
        pattern: String,
        /// Description of why the pattern is invalid.
        reason: String,
    },
}

/// Errors that can occur during configuration operations.
#[derive(Error, Debug)]
pub enum ConfigError {
    /// Configuration file could not be read.
    #[error("failed to read configuration file: {path}")]
    ReadError {
        /// Path to the configuration file.
        path: PathBuf,
        /// The underlying IO error.
        #[source]
        source: std::io::Error,
    },

    /// Configuration file could not be parsed.
    #[error("failed to parse configuration file: {path}")]
    ParseError {
        /// Path to the configuration file.
        path: PathBuf,
        /// The underlying JSON error.
        #[source]
        source: serde_json::Error,
    },

    /// Configuration file not found.
    #[error("configuration file not found: {path}")]
    NotFound {
        /// Path where the configuration file was expected.
        path: PathBuf,
    },

    /// Configuration validation failed.
    #[error("configuration validation failed: {message}")]
    ValidationError {
        /// Description of the validation failure.
        message: String,
    },

    /// Configuration file has insecure permissions.
    ///
    /// This is a warning on Unix systems when the config file is readable
    /// by other users, which could expose credentials.
    #[error("insecure file permissions on {path}: {message}")]
    InsecurePermissions {
        /// Path to the file with insecure permissions.
        path: PathBuf,
        /// Description of the permission issue.
        message: String,
    },

    /// SSH key file not found or inaccessible.
    #[error("SSH key file error for '{remote_name}': {message}")]
    SshKeyFileError {
        /// Name of the remote with the SSH key issue.
        remote_name: String,
        /// Description of the error.
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify that error messages do not accidentally include sensitive patterns.
    ///
    /// This is a basic sanity check. The real security comes from the type
    /// system preventing credentials from being included in error variants.
    #[test]
    fn error_messages_do_not_contain_credential_patterns() {
        let auth_error = AuthError::NoMatchingCredential {
            url: "https://github.com/user/repo".to_string(),
        };
        let msg = auth_error.to_string();

        // These patterns should never appear in error messages
        assert!(!msg.contains("ghp_"), "GitHub PAT prefix found in error");
        assert!(!msg.contains("glpat-"), "GitLab PAT prefix found in error");
        assert!(!msg.contains("-----BEGIN"), "SSH key found in error");
    }
}
