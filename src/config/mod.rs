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
//! # Example Configuration
//!
//! See `config/example-config.json` for a complete example.

mod settings;

pub use settings::{AiIdentity, Config, LoggingConfig, SecurityConfig};

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
/// # Errors
///
/// Returns an error if:
/// - The configuration file cannot be found
/// - The file cannot be read
/// - The JSON is malformed
/// - Required fields are missing or invalid
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

    Ok(config)
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
