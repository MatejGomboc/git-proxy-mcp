//! Configuration file loading and parsing.
//!
//! This module handles loading the configuration file from disk and parsing
//! it into validated, type-safe structures. Credentials are immediately
//! converted to secure types using the `secrecy` crate.
//!
//! # Configuration File Locations
//!
//! The configuration file is searched in the following order:
//!
//! 1. Path specified via `--config` CLI flag
//! 2. Default location:
//!    - **Linux/macOS:** `~/.git-proxy-mcp/config.json`
//!    - **Windows:** `%USERPROFILE%\.git-proxy-mcp\config.json`
//!
//! # Security
//!
//! On Unix systems, the configuration file and SSH key files are checked
//! for secure permissions. A warning is logged if files are readable by
//! other users, as this could expose credentials.
//!
//! # Example Configuration
//!
//! See `config/example-config.json` for a complete example.

mod settings;

pub use settings::{AiIdentity, AuthConfig, Config, LoggingConfig, RemoteConfig, SecurityConfig};

use std::path::{Path, PathBuf};

use crate::error::ConfigError;

/// Returns the default configuration directory.
///
/// - **Linux/macOS:** `~/.git-proxy-mcp/`
/// - **Windows:** `%USERPROFILE%\.git-proxy-mcp\`
#[must_use]
pub fn default_config_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|p| p.join(".git-proxy-mcp"))
}

/// Returns the platform-specific default configuration file path.
#[must_use]
pub fn default_config_path() -> Option<PathBuf> {
    default_config_dir().map(|p| p.join("config.json"))
}

/// Loads and parses the configuration file.
///
/// If `path` is `None`, uses the platform-specific default location.
///
/// # Security Checks
///
/// On Unix systems, this function checks file permissions and logs warnings
/// if files are readable by other users. The function will:
///
/// 1. Warn if the config file is world-readable or group-readable
/// 2. Validate that SSH key files exist and have secure permissions
///
/// # Errors
///
/// Returns an error if:
/// - The configuration file cannot be found
/// - The file cannot be read
/// - The JSON is malformed
/// - Required fields are missing or invalid
/// - SSH key files don't exist or are inaccessible
pub fn load_config(path: Option<&Path>) -> Result<Config, ConfigError> {
    let config_path = match path {
        Some(p) => p.to_path_buf(),
        None => default_config_path().ok_or_else(|| ConfigError::NotFound {
            path: PathBuf::from("<default config path>"),
        })?,
    };

    if !config_path.exists() {
        return Err(ConfigError::NotFound { path: config_path });
    }

    // Check config file permissions on Unix
    #[cfg(unix)]
    check_file_permissions(&config_path)?;

    let contents = std::fs::read_to_string(&config_path).map_err(|e| ConfigError::ReadError {
        path: config_path.clone(),
        source: e,
    })?;

    let config: Config = serde_json::from_str(&contents).map_err(|e| ConfigError::ParseError {
        path: config_path.clone(),
        source: e,
    })?;

    // Validate the configuration
    config.validate()?;

    // Validate SSH key files exist and have correct permissions
    validate_ssh_key_files(&config)?;

    Ok(config)
}

/// Checks if a file has secure permissions (Unix only).
///
/// Logs a warning if the file is readable by group or others.
/// This is important for files containing credentials.
///
/// # Errors
///
/// Returns an error if the file permissions cannot be read.
#[cfg(unix)]
fn check_file_permissions(path: &Path) -> Result<(), ConfigError> {
    use std::os::unix::fs::PermissionsExt;

    let metadata = std::fs::metadata(path).map_err(|e| ConfigError::ReadError {
        path: path.to_path_buf(),
        source: e,
    })?;

    let mode = metadata.permissions().mode();

    // Check for group-readable (0o040) or world-readable (0o004)
    let group_readable = mode & 0o040 != 0;
    let world_readable = mode & 0o004 != 0;

    if world_readable {
        tracing::warn!(
            path = %path.display(),
            mode = format!("{mode:o}"),
            "Configuration file is world-readable. This is a security risk as it may expose credentials. \
             Consider running: chmod 600 {}",
            path.display()
        );
    } else if group_readable {
        tracing::warn!(
            path = %path.display(),
            mode = format!("{mode:o}"),
            "Configuration file is group-readable. This may expose credentials to other users in your group. \
             Consider running: chmod 600 {}",
            path.display()
        );
    }

    Ok(())
}

/// Validates that all SSH key files in the configuration exist and have
/// secure permissions.
///
/// # Errors
///
/// Returns an error if an SSH key file:
/// - Does not exist
/// - Cannot be accessed
/// - Has insecure permissions (Unix only)
fn validate_ssh_key_files(config: &Config) -> Result<(), ConfigError> {
    for remote in &config.remotes {
        if let AuthConfig::SshKey { key_path, .. } = &remote.auth {
            let expanded_path = expand_tilde(key_path);

            // Check if file exists
            if !expanded_path.exists() {
                return Err(ConfigError::SshKeyFileError {
                    remote_name: remote.name.clone(),
                    message: format!("SSH key file not found: {}", expanded_path.display()),
                });
            }

            // Check if file is readable
            if std::fs::metadata(&expanded_path).is_err() {
                return Err(ConfigError::SshKeyFileError {
                    remote_name: remote.name.clone(),
                    message: format!("Cannot access SSH key file: {}", expanded_path.display()),
                });
            }

            // Check permissions on Unix
            #[cfg(unix)]
            check_ssh_key_permissions(&expanded_path, &remote.name)?;
        }
    }

    Ok(())
}

/// Checks if an SSH key file has secure permissions (Unix only).
///
/// SSH keys should only be readable by the owner (mode 0600 or 0400).
/// This function logs a warning if the permissions are too permissive.
///
/// # Errors
///
/// Returns an error if the file permissions indicate a serious security risk.
#[cfg(unix)]
fn check_ssh_key_permissions(path: &Path, remote_name: &str) -> Result<(), ConfigError> {
    use std::os::unix::fs::PermissionsExt;

    let metadata = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) => {
            return Err(ConfigError::SshKeyFileError {
                remote_name: remote_name.to_string(),
                message: format!("Cannot read SSH key file metadata: {e}"),
            });
        }
    };

    let mode = metadata.permissions().mode();

    // Check for group-readable (0o040) or world-readable (0o004)
    let group_readable = mode & 0o040 != 0;
    let world_readable = mode & 0o004 != 0;
    let group_writable = mode & 0o020 != 0;
    let world_writable = mode & 0o002 != 0;

    if world_readable || world_writable {
        // This is a serious security issue - SSH will likely reject the key anyway
        return Err(ConfigError::SshKeyFileError {
            remote_name: remote_name.to_string(),
            message: format!(
                "SSH key file {} has insecure permissions (mode {:o}). \
                 SSH keys must not be readable or writable by others. \
                 Run: chmod 600 {}",
                path.display(),
                mode & 0o777,
                path.display()
            ),
        });
    }

    if group_readable || group_writable {
        tracing::warn!(
            path = %path.display(),
            remote = %remote_name,
            mode = format!("{:o}", mode & 0o777),
            "SSH key file is accessible by group. This may be rejected by SSH. \
             Consider running: chmod 600 {}",
            path.display()
        );
    }

    Ok(())
}

/// Expands `~` to the user's home directory in a path string.
///
/// Returns the original path if `~` expansion fails or is not needed.
#[must_use]
pub fn expand_tilde(path: &str) -> PathBuf {
    if let Some(stripped) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(stripped);
        }
    } else if path == "~" {
        if let Some(home) = dirs::home_dir() {
            return home;
        }
    }
    PathBuf::from(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_tilde_with_path() {
        let expanded = expand_tilde("~/.ssh/id_ed25519");
        // Should not start with ~ anymore
        assert!(!expanded.to_string_lossy().starts_with('~'));
        // Should end with the original suffix
        assert!(expanded.to_string_lossy().ends_with(".ssh/id_ed25519"));
    }

    #[test]
    fn expand_tilde_alone() {
        let expanded = expand_tilde("~");
        assert!(!expanded.to_string_lossy().starts_with('~'));
    }

    #[test]
    fn expand_tilde_no_tilde() {
        let path = "/absolute/path/to/key";
        let expanded = expand_tilde(path);
        assert_eq!(expanded, PathBuf::from(path));
    }

    #[test]
    fn default_config_dir_exists() {
        // This should return Some on all platforms
        assert!(default_config_dir().is_some());
    }

    #[test]
    fn default_config_path_exists() {
        let path = default_config_path();
        assert!(path.is_some());
        assert!(path.unwrap().to_string_lossy().contains("config.json"));
    }
}
