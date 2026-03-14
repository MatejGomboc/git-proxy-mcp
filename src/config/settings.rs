//! Configuration structures for deserialisation.
//!
//! These structures map directly to the JSON configuration file format.
//! The MCP server no longer stores credentials — it relies on the user's
//! existing Git configuration.

use std::path::PathBuf;
use std::time::Duration;

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

    /// Optional additional comment field (ignored during parsing).
    #[serde(rename = "_note", default)]
    _note: Option<String>,

    /// Security settings.
    #[serde(default)]
    pub security: SecurityConfig,

    /// Logging settings.
    #[serde(default)]
    pub logging: LoggingConfig,

    /// Timeout settings.
    #[serde(default)]
    pub timeouts: TimeoutConfig,

    /// Limits settings.
    #[serde(default)]
    pub limits: LimitsConfig,

    /// Rate limiting settings.
    #[serde(default)]
    pub rate_limits: RateLimitConfig,

    /// Git identity settings for AI-assisted commits.
    #[serde(default)]
    pub git_identity: GitIdentityConfig,

    /// Proxy settings for network connections.
    #[serde(default)]
    pub proxy: ProxyConfig,

    /// Session management settings.
    #[serde(default)]
    pub sessions: SessionConfig,
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

/// Default request timeout in seconds.
const fn default_request_timeout_secs() -> u64 {
    300 // 5 minutes
}

/// Default maximum output size in bytes (10 MiB).
const fn default_max_output_bytes() -> usize {
    10 * 1024 * 1024
}

/// Default maximum burst for rate limiting.
const fn default_rate_limit_max_burst() -> u64 {
    20
}

/// Default refill rate for rate limiting (operations per second).
const fn default_rate_limit_refill_rate() -> f64 {
    5.0
}

/// Timeout configuration.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimeoutConfig {
    /// Timeout for git command execution in seconds.
    ///
    /// If a git command takes longer than this, it will be terminated.
    /// This prevents hung git processes from blocking the server indefinitely.
    ///
    /// Default: 300 seconds (5 minutes).
    #[serde(default = "default_request_timeout_secs")]
    pub request_timeout_secs: u64,
}

impl Default for TimeoutConfig {
    fn default() -> Self {
        Self {
            request_timeout_secs: default_request_timeout_secs(),
        }
    }
}

impl TimeoutConfig {
    /// Returns the request timeout as a `Duration`.
    #[must_use]
    pub const fn request_timeout(&self) -> Duration {
        Duration::from_secs(self.request_timeout_secs)
    }
}

/// Limits configuration.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LimitsConfig {
    /// Maximum output size in bytes.
    ///
    /// If the combined stdout and stderr output from a git command exceeds
    /// this limit, the output will be truncated and a warning added.
    /// This prevents protocol buffer overflow when processing large outputs.
    ///
    /// Default: 10 MiB (10,485,760 bytes).
    #[serde(default = "default_max_output_bytes")]
    pub max_output_bytes: usize,
}

impl Default for LimitsConfig {
    fn default() -> Self {
        Self {
            max_output_bytes: default_max_output_bytes(),
        }
    }
}

impl LimitsConfig {
    /// Returns the maximum output size in bytes.
    #[must_use]
    pub const fn max_output_bytes(&self) -> usize {
        self.max_output_bytes
    }
}

/// Git identity configuration.
///
/// Allows setting a custom Git identity for AI-assisted commits.
/// This helps distinguish commits made by the AI from those made by humans,
/// improving auditability and attribution in the git history.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct GitIdentityConfig {
    /// The name to use for commit author/committer.
    ///
    /// Example: "Claude AI"
    #[serde(default)]
    pub name: Option<String>,

    /// The email to use for commit author/committer.
    ///
    /// Example: "ai-assistant@your-domain.com"
    #[serde(default)]
    pub email: Option<String>,
}

/// Rate limiting configuration.
///
/// Controls how many Git commands can be executed per unit of time.
/// Uses a token bucket algorithm where operations consume tokens and
/// tokens are replenished at a steady rate.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RateLimitConfig {
    /// Maximum number of operations allowed in a burst.
    ///
    /// This is the maximum number of Git commands that can be executed
    /// in rapid succession before rate limiting kicks in.
    ///
    /// Default: 20
    #[serde(default = "default_rate_limit_max_burst")]
    pub max_burst: u64,

    /// Sustained rate of operations allowed per second.
    ///
    /// After the burst capacity is exhausted, this is the maximum
    /// sustained rate of Git commands that can be executed.
    ///
    /// Default: 5.0 (operations per second)
    #[serde(default = "default_rate_limit_refill_rate")]
    pub refill_rate_per_sec: f64,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            max_burst: default_rate_limit_max_burst(),
            refill_rate_per_sec: default_rate_limit_refill_rate(),
        }
    }
}

/// Default session timeout in seconds (1 hour).
const fn default_session_timeout_secs() -> u64 {
    3600
}

/// Default maximum concurrent Tier 2 streaming sessions.
const fn default_max_streaming_sessions() -> usize {
    10
}

/// Default maximum concurrent repo tracking sessions.
const fn default_max_repo_sessions() -> usize {
    100
}

/// Session management configuration.
///
/// Controls timeouts and concurrency limits for both Tier 2 streaming
/// sessions (chunked clone) and repo tracking sessions.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionConfig {
    /// Timeout for inactive sessions in seconds.
    ///
    /// Sessions that have not been accessed within this period are
    /// automatically cleaned up.
    ///
    /// Default: 3600 (1 hour).
    #[serde(default = "default_session_timeout_secs")]
    pub timeout_secs: u64,

    /// Maximum number of concurrent Tier 2 streaming sessions.
    ///
    /// Limits how many chunked clone operations can be in progress
    /// simultaneously.
    ///
    /// Default: 10.
    #[serde(default = "default_max_streaming_sessions")]
    pub max_streaming_sessions: usize,

    /// Maximum number of concurrent repo tracking sessions.
    ///
    /// Default: 100.
    #[serde(default = "default_max_repo_sessions")]
    pub max_repo_sessions: usize,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            timeout_secs: default_session_timeout_secs(),
            max_streaming_sessions: default_max_streaming_sessions(),
            max_repo_sessions: default_max_repo_sessions(),
        }
    }
}

impl SessionConfig {
    /// Returns the session timeout as a `Duration`.
    #[must_use]
    pub const fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_secs)
    }
}

/// Proxy configuration for network connections.
///
/// When configured, all git fetch/push/connect operations and LFS HTTP
/// requests will be routed through the specified proxy server.
/// If no proxy is configured, git2 falls back to auto-detection from
/// the user's git config (`http.proxy`).
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ProxyConfig {
    /// Proxy URL (e.g., `"http://proxy.example.com:8080"`,
    /// `"socks5://proxy.example.com:1080"`).
    ///
    /// Supports HTTP, HTTPS, and SOCKS5 proxy protocols.
    #[serde(default)]
    pub url: Option<String>,

    /// Comma-separated list of hosts that should bypass the proxy.
    ///
    /// Supports wildcards (e.g., `"*.internal.com,localhost,127.0.0.1"`).
    #[serde(default)]
    pub no_proxy: Option<String>,
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

    #[test]
    fn timeout_config_defaults() {
        let config = TimeoutConfig::default();
        assert_eq!(config.request_timeout_secs, 300);
        assert_eq!(config.request_timeout(), Duration::from_secs(300));
    }

    #[test]
    fn parse_timeout_config() {
        let json = r#"{
            "timeouts": {
                "request_timeout_secs": 60
            }
        }"#;

        let config: Config = serde_json::from_str(json).unwrap();
        assert_eq!(config.timeouts.request_timeout_secs, 60);
        assert_eq!(config.timeouts.request_timeout(), Duration::from_secs(60));
    }

    #[test]
    fn parse_full_config_with_timeouts() {
        let json = r#"{
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "_comment": "Test config",
            "_note": "Additional note",
            "security": {
                "allow_force_push": false,
                "protected_branches": ["main"]
            },
            "logging": {
                "level": "debug"
            },
            "timeouts": {
                "request_timeout_secs": 120
            }
        }"#;

        let config: Config = serde_json::from_str(json).unwrap();
        assert!(config.validate().is_ok());
        assert_eq!(config.timeouts.request_timeout_secs, 120);
    }

    #[test]
    fn limits_config_defaults() {
        let config = LimitsConfig::default();
        assert_eq!(config.max_output_bytes, 10 * 1024 * 1024);
        assert_eq!(config.max_output_bytes(), 10 * 1024 * 1024);
    }

    #[test]
    fn parse_limits_config() {
        let json = r#"{
            "limits": {
                "max_output_bytes": 5242880
            }
        }"#;

        let config: Config = serde_json::from_str(json).unwrap();
        assert_eq!(config.limits.max_output_bytes, 5 * 1024 * 1024);
        assert_eq!(config.limits.max_output_bytes(), 5 * 1024 * 1024);
    }

    #[test]
    fn parse_full_config_with_limits() {
        let json = r#"{
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "_comment": "Test config",
            "security": {
                "allow_force_push": false
            },
            "logging": {
                "level": "info"
            },
            "timeouts": {
                "request_timeout_secs": 60
            },
            "limits": {
                "max_output_bytes": 1048576
            }
        }"#;

        let config: Config = serde_json::from_str(json).unwrap();
        assert!(config.validate().is_ok());
        assert_eq!(config.limits.max_output_bytes, 1024 * 1024);
    }

    #[test]
    fn rate_limit_config_defaults() {
        let config = RateLimitConfig::default();
        assert_eq!(config.max_burst, 20);
        assert!((config.refill_rate_per_sec - 5.0).abs() < f64::EPSILON);
    }

    #[test]
    fn parse_rate_limit_config() {
        let json = r#"{
            "rate_limits": {
                "max_burst": 50,
                "refill_rate_per_sec": 10.0
            }
        }"#;

        let config: Config = serde_json::from_str(json).unwrap();
        assert_eq!(config.rate_limits.max_burst, 50);
        assert!((config.rate_limits.refill_rate_per_sec - 10.0).abs() < f64::EPSILON);
    }

    #[test]
    fn parse_rate_limit_partial_config() {
        let json = r#"{
            "rate_limits": {
                "max_burst": 100
            }
        }"#;

        let config: Config = serde_json::from_str(json).unwrap();
        assert_eq!(config.rate_limits.max_burst, 100);
        // Should use default for refill_rate_per_sec
        assert!((config.rate_limits.refill_rate_per_sec - 5.0).abs() < f64::EPSILON);
    }

    #[test]
    fn parse_full_config_with_rate_limits() {
        let json = r#"{
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "_comment": "Test config",
            "security": {
                "allow_force_push": false,
                "protected_branches": ["main"]
            },
            "logging": {
                "level": "debug"
            },
            "timeouts": {
                "request_timeout_secs": 120
            },
            "rate_limits": {
                "max_burst": 30,
                "refill_rate_per_sec": 8.0
            }
        }"#;

        let config: Config = serde_json::from_str(json).unwrap();
        assert!(config.validate().is_ok());
        assert_eq!(config.rate_limits.max_burst, 30);
        assert!((config.rate_limits.refill_rate_per_sec - 8.0).abs() < f64::EPSILON);
    }

    #[test]
    fn git_identity_config_defaults() {
        let config = GitIdentityConfig::default();
        assert!(config.name.is_none());
        assert!(config.email.is_none());
    }

    #[test]
    fn parse_git_identity_config() {
        let json = r#"{
            "git_identity": {
                "name": "Claude AI",
                "email": "ai-assistant@example.com"
            }
        }"#;

        let config: Config = serde_json::from_str(json).unwrap();
        assert_eq!(config.git_identity.name, Some("Claude AI".to_string()));
        assert_eq!(
            config.git_identity.email,
            Some("ai-assistant@example.com".to_string())
        );
    }

    #[test]
    fn parse_git_identity_partial_config() {
        let json = r#"{
            "git_identity": {
                "name": "AI Bot"
            }
        }"#;

        let config: Config = serde_json::from_str(json).unwrap();
        assert_eq!(config.git_identity.name, Some("AI Bot".to_string()));
        assert!(config.git_identity.email.is_none());
    }

    #[test]
    fn proxy_config_defaults() {
        let config = ProxyConfig::default();
        assert!(config.url.is_none());
        assert!(config.no_proxy.is_none());
    }

    #[test]
    fn parse_proxy_config() {
        let json = r#"{
            "proxy": {
                "url": "http://proxy.example.com:8080",
                "no_proxy": "*.internal.com,localhost"
            }
        }"#;

        let config: Config = serde_json::from_str(json).unwrap();
        assert_eq!(
            config.proxy.url,
            Some("http://proxy.example.com:8080".to_string())
        );
        assert_eq!(
            config.proxy.no_proxy,
            Some("*.internal.com,localhost".to_string())
        );
    }

    #[test]
    fn parse_proxy_url_only() {
        let json = r#"{
            "proxy": {
                "url": "socks5://proxy.example.com:1080"
            }
        }"#;

        let config: Config = serde_json::from_str(json).unwrap();
        assert_eq!(
            config.proxy.url,
            Some("socks5://proxy.example.com:1080".to_string())
        );
        assert!(config.proxy.no_proxy.is_none());
    }

    #[test]
    fn parse_full_config_with_git_identity() {
        let json = r#"{
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "_comment": "Test config",
            "security": {
                "allow_force_push": false,
                "protected_branches": ["main"]
            },
            "logging": {
                "level": "debug"
            },
            "git_identity": {
                "name": "Claude AI",
                "email": "claude@anthropic.com"
            }
        }"#;

        let config: Config = serde_json::from_str(json).unwrap();
        assert!(config.validate().is_ok());
        assert_eq!(config.git_identity.name, Some("Claude AI".to_string()));
        assert_eq!(
            config.git_identity.email,
            Some("claude@anthropic.com".to_string())
        );
    }
}
