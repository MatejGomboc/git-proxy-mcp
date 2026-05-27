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

    #[test]
    fn load_config_with_directory_path_returns_read_error() {
        // `Path::exists()` is true for a directory, so the existence check
        // passes — but `read_to_string` cannot read a directory as a file.
        // This must surface as ReadError (not NotFound, and not a panic).
        let dir = tempfile::tempdir().unwrap();
        let result = load_config(Some(dir.path()));
        assert!(matches!(result, Err(ConfigError::ReadError { .. })));
    }

    #[test]
    fn load_config_none_resolves_default_path() {
        // With no explicit path, load_config falls back to the platform
        // default location. We cannot control whether a real config exists on
        // the machine running the tests, so assert the two legitimate
        // outcomes: an existing file loads (or fails to parse, but is never
        // reported as missing), and an absent file yields NotFound.
        let result = load_config(None);
        match default_config_path() {
            Some(path) if path.is_file() => {
                assert!(!matches!(result, Err(ConfigError::NotFound { .. })));
            }
            _ => {
                assert!(matches!(result, Err(ConfigError::NotFound { .. })));
            }
        }
    }

    #[test]
    fn load_config_parses_shipped_example_config() {
        // The shipped `config/example-config.json` must always parse against
        // the current `Config` struct. Because every section uses
        // `deny_unknown_fields`, this fails if the example carries a field the
        // struct dropped (e.g. the removed `lfs.max_total_size`) or omits a
        // section that lost its `#[serde(default)]` — a guard against the
        // config-drift bug class. The example also documents the built-in
        // defaults, so we pin each default-valued section against the code
        // default: a change to the code default OR the example file (but not
        // both) is then caught here.
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("config")
            .join("example-config.json");
        let config = load_config(Some(&path))
            .expect("shipped config/example-config.json must parse and validate");

        // git_identity: the example shows the documented sample identity.
        assert_eq!(config.git_identity.name.as_deref(), Some("Claude AI"));
        assert_eq!(
            config.git_identity.email.as_deref(),
            Some("ai-assistant@your-domain.com")
        );

        // security: only protected_branches deviates from the default (the
        // example recommends an explicit ["main", "master"]); the rest match.
        let security_default = SecurityConfig::default();
        assert_eq!(
            config.security.allow_force_push,
            security_default.allow_force_push
        );
        assert_eq!(config.security.protected_branches, ["main", "master"]);
        assert_eq!(
            config.security.repo_allowlist,
            security_default.repo_allowlist
        );
        assert_eq!(
            config.security.repo_blocklist,
            security_default.repo_blocklist
        );

        // The remaining sections show pure defaults — tie them to the code.
        let logging_default = LoggingConfig::default();
        assert_eq!(config.logging.level, logging_default.level);
        assert_eq!(
            config.logging.audit_log_path,
            logging_default.audit_log_path
        );

        let timeouts_default = TimeoutConfig::default();
        assert_eq!(
            config.timeouts.request_timeout_secs,
            timeouts_default.request_timeout_secs
        );

        let sessions_default = SessionConfig::default();
        assert_eq!(config.sessions.timeout_secs, sessions_default.timeout_secs);
        assert_eq!(
            config.sessions.max_streaming_sessions,
            sessions_default.max_streaming_sessions
        );
        assert_eq!(
            config.sessions.max_repo_sessions,
            sessions_default.max_repo_sessions
        );

        let lfs_default = LfsConfig::default();
        assert_eq!(
            config.lfs.retry_max_attempts,
            lfs_default.retry_max_attempts
        );
        assert_eq!(
            config.lfs.retry_initial_backoff_ms,
            lfs_default.retry_initial_backoff_ms
        );
        assert_eq!(
            config.lfs.retry_max_backoff_ms,
            lfs_default.retry_max_backoff_ms
        );
        assert!(
            (config.lfs.retry_backoff_multiplier - lfs_default.retry_backoff_multiplier).abs()
                < f64::EPSILON
        );
        assert_eq!(config.lfs.max_object_size, lfs_default.max_object_size);
        assert_eq!(
            config.lfs.request_timeout_secs,
            lfs_default.request_timeout_secs
        );
        assert_eq!(
            config.lfs.connect_timeout_secs,
            lfs_default.connect_timeout_secs
        );
        assert_eq!(
            config.lfs.download_timeout_secs,
            lfs_default.download_timeout_secs
        );

        let submodules_default = SubmoduleConfig::default();
        assert_eq!(
            config.submodules.max_concurrent,
            submodules_default.max_concurrent
        );
        assert_eq!(
            config.submodules.max_failures,
            submodules_default.max_failures
        );
        assert_eq!(
            config.submodules.include_patterns,
            submodules_default.include_patterns
        );
        assert_eq!(
            config.submodules.exclude_patterns,
            submodules_default.exclude_patterns
        );

        let proxy_default = ProxyConfig::default();
        assert_eq!(config.proxy.url, proxy_default.url);
        assert_eq!(config.proxy.no_proxy, proxy_default.no_proxy);

        // limits / rate_limits types are not re-exported from this module, so
        // assert the documented literal defaults directly.
        assert_eq!(config.limits.max_output_bytes, 10 * 1024 * 1024);
        assert_eq!(config.rate_limits.max_burst, 20);
        assert!((config.rate_limits.refill_rate_per_sec - 5.0).abs() < f64::EPSILON);
    }
}
