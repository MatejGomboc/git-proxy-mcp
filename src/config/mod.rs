//! Configuration file loading and parsing.
//!
//! This module handles loading the configuration file from disk and parsing
//! it into validated, type-safe structures.
//!
//! # Architecture
//!
//! The MCP server does NOT store credentials. It relies on the user's
//! existing Git configuration (credential helpers, SSH agent, etc.).
//! The config file only contains security and logging settings.
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

pub use settings::{
    Config, LfsConfig, LoggingConfig, ProxyConfig, SecurityConfig, SessionConfig, SubmoduleConfig,
    TimeoutConfig,
};

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

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn load_config_with_nonexistent_path_returns_not_found() {
        let path = std::path::Path::new("/nonexistent/path/config.json");
        let result = load_config(Some(path));
        assert!(matches!(result, Err(ConfigError::NotFound { .. })));
    }

    #[test]
    fn load_config_parses_valid_json() {
        let temp = tempfile::NamedTempFile::new().unwrap();
        let json = r#"{
            "git_identity": {"name": "Test", "email": "t@x.com"},
            "security": {},
            "logging": {},
            "timeouts": {},
            "rate_limits": {}
        }"#;
        std::fs::write(temp.path(), json).unwrap();

        let config = load_config(Some(temp.path())).unwrap();
        assert_eq!(config.git_identity.name.as_deref(), Some("Test"));
    }

    #[test]
    fn load_config_rejects_malformed_json() {
        let temp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(temp.path(), "{ not valid json").unwrap();

        let result = load_config(Some(temp.path()));
        assert!(matches!(result, Err(ConfigError::ParseError { .. })));
    }

    #[test]
    fn load_config_rejects_unknown_field() {
        let temp = tempfile::NamedTempFile::new().unwrap();
        // unknown field at top level
        let json = r#"{"unknown_field": 123}"#;
        std::fs::write(temp.path(), json).unwrap();

        let result = load_config(Some(temp.path()));
        assert!(matches!(result, Err(ConfigError::ParseError { .. })));
    }

    #[test]
    fn load_config_minimal_empty_object() {
        let temp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(temp.path(), "{}").unwrap();

        let config = load_config(Some(temp.path())).unwrap();
        // All defaults
        assert!(config.git_identity.name.is_none());
    }

    #[test]
    fn load_config_with_full_configuration() {
        let temp = tempfile::NamedTempFile::new().unwrap();
        let json = r#"{
            "git_identity": {"name": "AI", "email": "ai@x.com"},
            "security": {"allow_force_push": false, "protected_branches": ["main"]},
            "logging": {"level": "info"},
            "timeouts": {"request_timeout_secs": 60},
            "rate_limits": {"max_burst": 50, "refill_rate_per_sec": 10.0},
            "proxy": {"url": "http://proxy:8080"},
            "sessions": {"timeout_secs": 1800, "max_streaming_sessions": 5, "max_repo_sessions": 50},
            "lfs": {"retry_max_attempts": 5},
            "submodules": {"max_concurrent": 2}
        }"#;
        std::fs::write(temp.path(), json).unwrap();
        let config = load_config(Some(temp.path())).unwrap();
        assert_eq!(config.timeouts.request_timeout_secs, 60);
    }
}
