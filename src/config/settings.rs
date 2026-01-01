//! Configuration structures for deserialisation.
//!
//! These structures map directly to the JSON configuration file format.
//! The MCP server no longer stores credentials — it relies on the user's
//! existing Git configuration.

use std::path::PathBuf;

use serde::Deserialize;

use crate::error::ConfigError;

/// Root configuration structure.
///
/// This is the top-level structure that matches the JSON config file.
/// Note: Credentials are NOT stored in the config file. The MCP server
/// relies on the user's existing Git configuration (credential helpers,
/// SSH agent, etc.).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Optional JSON schema reference (ignored during parsing).
    #[serde(rename = "$schema", default)]
    _schema: Option<String>,

    /// Optional comment field (ignored during parsing).
    #[serde(rename = "_comment", default)]
    _comment: Option<String>,

    /// Security settings.
    #[serde(default)]
    pub security: SecurityConfig,

    /// Logging settings.
    #[serde(default)]
    pub logging: LoggingConfig,
}

impl Config {
    /// Validates the configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if any validation checks fail.
    pub const fn validate(&self) -> Result<(), ConfigError> {
        // Currently no validation required for security/logging settings
        // as they all have sensible defaults
        Ok(())
    }
}

/// Security configuration.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct SecurityConfig {
    /// Whether to allow force pushes.
    #[serde(default)]
    pub allow_force_push: bool,

    /// List of protected branch names.
    #[serde(default)]
    pub protected_branches: Vec<String>,

    /// Optional allowlist of repository patterns.
    #[serde(default)]
    pub repo_allowlist: Option<Vec<String>>,

    /// Optional blocklist of repository patterns.
    #[serde(default)]
    pub repo_blocklist: Option<Vec<String>>,
}

/// Logging configuration.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoggingConfig {
    /// Log level (trace, debug, info, warn, error).
    #[serde(default = "default_log_level")]
    pub level: String,

    /// Optional path to audit log file.
    #[serde(default)]
    pub audit_log_path: Option<PathBuf>,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: default_log_level(),
            audit_log_path: None,
        }
    }
}

/// Default log level.
fn default_log_level() -> String {
    "warn".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal_config() {
        let json = r"{}";

        let config: Config = serde_json::from_str(json).unwrap();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn parse_full_config() {
        let json = r#"{
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "_comment": "Test config",
            "security": {
                "allow_force_push": false,
                "protected_branches": ["main", "master"],
                "repo_allowlist": ["https://github.com/myorg/*"],
                "repo_blocklist": ["https://github.com/public/*"]
            },
            "logging": {
                "level": "debug",
                "audit_log_path": "/var/log/git-proxy-mcp.log"
            }
        }"#;

        let config: Config = serde_json::from_str(json).unwrap();
        assert!(config.validate().is_ok());
        assert!(!config.security.allow_force_push);
        assert_eq!(config.security.protected_branches.len(), 2);
        assert!(config.security.repo_allowlist.is_some());
        assert!(config.security.repo_blocklist.is_some());
        assert_eq!(config.logging.level, "debug");
        assert!(config.logging.audit_log_path.is_some());
    }

    #[test]
    fn security_config_defaults() {
        let config = SecurityConfig::default();
        assert!(!config.allow_force_push);
        assert!(config.protected_branches.is_empty());
        assert!(config.repo_allowlist.is_none());
        assert!(config.repo_blocklist.is_none());
    }

    #[test]
    fn logging_config_defaults() {
        let config = LoggingConfig::default();
        assert_eq!(config.level, "warn");
        assert!(config.audit_log_path.is_none());
    }

    #[test]
    fn parse_security_only() {
        let json = r#"{
            "security": {
                "allow_force_push": true,
                "protected_branches": ["release/*"]
            }
        }"#;

        let config: Config = serde_json::from_str(json).unwrap();
        assert!(config.security.allow_force_push);
        assert_eq!(config.security.protected_branches, vec!["release/*"]);
    }

    #[test]
    fn parse_logging_only() {
        let json = r#"{
            "logging": {
                "level": "trace"
            }
        }"#;

        let config: Config = serde_json::from_str(json).unwrap();
        assert_eq!(config.logging.level, "trace");
    }

    #[test]
    fn reject_unknown_fields() {
        let json = r#"{
            "unknown_field": "value"
        }"#;

        let result: Result<Config, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }
}
