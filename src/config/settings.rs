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

    // Field order matches the user-facing layout in `config/example-config.json`
    // and the README configuration table, so a reader who has the JSON file or
    // the README open sees the struct laid out the same way. Serde keys on
    // field NAMES (not positions), so reordering doesn't affect deserialisation.
    /// Git identity settings for AI-assisted commits.
    #[serde(default)]
    pub git_identity: GitIdentityConfig,

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

    /// Proxy settings for network connections.
    #[serde(default)]
    pub proxy: ProxyConfig,

    /// Session management settings.
    #[serde(default)]
    pub sessions: SessionConfig,

    /// Git LFS settings.
    #[serde(default)]
    pub lfs: LfsConfig,

    /// Submodule settings.
    #[serde(default)]
    pub submodules: SubmoduleConfig,
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
    ///
    /// Default: `false`. When `false`, `--force` / `-f` /
    /// `--force-with-lease` pushes are rejected by `PushGuard::check`.
    #[serde(default)]
    pub allow_force_push: bool,

    /// List of protected branch names.
    ///
    /// Branches in this list block force-push and deletion attempts.
    /// Wildcard patterns (e.g. `release/*`) are supported by the matcher.
    ///
    /// Default: empty list, which `McpServer::new` treats as "use the
    /// built-in safe set" — `BranchGuard::with_defaults()` substitutes
    /// `main`, `master`, `develop`. Setting any non-empty list overrides
    /// the fallback (so `["main"]` protects only `main`, not also
    /// `master`/`develop`).
    #[serde(default)]
    pub protected_branches: Vec<String>,

    /// Optional allowlist of repository patterns.
    ///
    /// If `Some`, only repository URLs matching at least one pattern are
    /// allowed (allowlist mode). If `None`, allowlist mode is disabled
    /// and any URL not on the blocklist is allowed.
    ///
    /// Default: `None` (allowlist mode disabled).
    #[serde(default)]
    pub repo_allowlist: Option<Vec<String>>,

    /// Optional blocklist of repository patterns.
    ///
    /// If `Some`, repository URLs matching any pattern are rejected
    /// (blocklist mode). Takes precedence over the allowlist.
    ///
    /// Default: `None` (no blocklist).
    #[serde(default)]
    pub repo_blocklist: Option<Vec<String>>,
}

/// Logging configuration.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoggingConfig {
    /// Log level: one of `trace`, `debug`, `info`, `warn`, `error`.
    ///
    /// Default: `"warn"` — only warnings and errors are emitted, keeping
    /// the JSON-RPC channel quiet for normal operation.
    #[serde(default = "default_log_level")]
    pub level: String,

    /// Optional path to audit log file.
    ///
    /// When set, every Git operation, security-guard decision, and
    /// rate-limit event is appended to this file as a JSON line —
    /// see `src/security/audit.rs` for the schema.
    ///
    /// Default: `None` (audit logging disabled).
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

/// Default LFS retry max attempts.
const fn default_lfs_retry_max_attempts() -> u32 {
    3
}

/// Default LFS retry initial backoff in milliseconds.
const fn default_lfs_retry_initial_backoff_ms() -> u64 {
    500
}

/// Default LFS retry max backoff in milliseconds.
const fn default_lfs_retry_max_backoff_ms() -> u64 {
    30_000
}

/// Default LFS retry backoff multiplier.
const fn default_lfs_retry_backoff_multiplier() -> f64 {
    2.0
}

/// Default LFS HTTP request timeout (seconds).
///
/// Caps total time for any single LFS HTTP request (Batch API call or
/// object download). Without this, a hung server can stall the entire
/// MCP operation indefinitely.
const fn default_lfs_request_timeout_secs() -> u64 {
    300
}

/// Default LFS HTTP connect timeout (seconds).
const fn default_lfs_connect_timeout_secs() -> u64 {
    30
}

/// Default LFS HTTP per-object download timeout (seconds).
///
/// Object downloads can take much longer than the Batch API call
/// (multi-GiB blobs, slow CDNs), so they get their own (typically
/// larger) cap. Applied via `RequestBuilder::timeout` per request,
/// which overrides the `Client::builder().timeout` default.
const fn default_lfs_download_timeout_secs() -> u64 {
    600
}

/// Git LFS configuration.
///
/// Controls retry behaviour, size limits, and download settings
/// for LFS object fetching.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LfsConfig {
    /// Maximum number of retry attempts for failed LFS downloads.
    ///
    /// Only transient errors (HTTP 429, 500, 502, 503, 504, connection
    /// errors) are retried. Client errors (401, 403, 404) are not.
    ///
    /// Default: 3.
    #[serde(default = "default_lfs_retry_max_attempts")]
    pub retry_max_attempts: u32,

    /// Initial backoff delay in milliseconds before the first retry.
    ///
    /// Subsequent retries use exponential backoff up to `retry_max_backoff_ms`.
    ///
    /// Default: 500.
    #[serde(default = "default_lfs_retry_initial_backoff_ms")]
    pub retry_initial_backoff_ms: u64,

    /// Maximum backoff delay in milliseconds between retries.
    ///
    /// Default: 30000 (30 seconds).
    #[serde(default = "default_lfs_retry_max_backoff_ms")]
    pub retry_max_backoff_ms: u64,

    /// Multiplier applied to backoff delay after each retry.
    ///
    /// Default: 2.0.
    #[serde(default = "default_lfs_retry_backoff_multiplier")]
    pub retry_backoff_multiplier: f64,

    /// Maximum size in bytes for a single LFS object.
    ///
    /// Objects exceeding this limit are skipped (the pointer file is
    /// included in the archive instead). Set to `null` for unlimited.
    ///
    /// Default: unlimited.
    #[serde(default)]
    pub max_object_size: Option<u64>,

    /// HTTP request timeout in seconds (per LFS request).
    ///
    /// Caps total time for any single LFS HTTP request (Batch API call
    /// or object download). Without this, a hung LFS server can stall
    /// the entire MCP operation indefinitely.
    ///
    /// Default: 300 (5 minutes).
    #[serde(default = "default_lfs_request_timeout_secs")]
    pub request_timeout_secs: u64,

    /// HTTP connect timeout in seconds.
    ///
    /// Caps the time spent establishing a TCP+TLS connection to the LFS
    /// server, before the request itself is sent.
    ///
    /// Default: 30.
    #[serde(default = "default_lfs_connect_timeout_secs")]
    pub connect_timeout_secs: u64,

    /// HTTP per-object download timeout in seconds.
    ///
    /// Object downloads can take much longer than the Batch API call
    /// (multi-GiB blobs, slow CDNs), so they get their own (typically
    /// larger) cap. Applied via `RequestBuilder::timeout` for the
    /// individual GET, which overrides the `Client::builder().timeout`
    /// default used for the Batch API POST.
    ///
    /// Default: 600 (10 minutes).
    #[serde(default = "default_lfs_download_timeout_secs")]
    pub download_timeout_secs: u64,
}

impl Default for LfsConfig {
    fn default() -> Self {
        Self {
            retry_max_attempts: default_lfs_retry_max_attempts(),
            retry_initial_backoff_ms: default_lfs_retry_initial_backoff_ms(),
            retry_max_backoff_ms: default_lfs_retry_max_backoff_ms(),
            retry_backoff_multiplier: default_lfs_retry_backoff_multiplier(),
            max_object_size: None,
            request_timeout_secs: default_lfs_request_timeout_secs(),
            connect_timeout_secs: default_lfs_connect_timeout_secs(),
            download_timeout_secs: default_lfs_download_timeout_secs(),
        }
    }
}

/// Default maximum concurrent submodule fetches.
const fn default_submodule_max_concurrent() -> usize {
    4
}

/// Default maximum submodule fetch failures before stopping.
const fn default_submodule_max_failures() -> usize {
    3
}

/// Submodule configuration.
///
/// Controls filtering and concurrency for submodule fetching.
/// Recursion depth is a per-request argument on the MCP tool,
/// mirroring how `--recurse-submodules` works in Git.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubmoduleConfig {
    /// Maximum number of submodules fetched in parallel.
    ///
    /// Default: 4.
    #[serde(default = "default_submodule_max_concurrent")]
    pub max_concurrent: usize,

    /// Maximum number of submodule fetch failures before stopping.
    ///
    /// Once this many submodules fail, the remaining are skipped.
    ///
    /// Default: 3.
    #[serde(default = "default_submodule_max_failures")]
    pub max_failures: usize,

    /// Glob patterns for submodule paths to include.
    ///
    /// If set, only submodules matching at least one pattern are fetched.
    /// Example: `["lib/*", "deps/core"]`.
    #[serde(default)]
    pub include_patterns: Option<Vec<String>>,

    /// Glob patterns for submodule paths to exclude.
    ///
    /// Submodules matching any pattern are skipped. Exclusions take
    /// precedence over inclusions.
    /// Example: `["vendor/*", "third_party/*"]`.
    #[serde(default)]
    pub exclude_patterns: Option<Vec<String>>,
}

impl Default for SubmoduleConfig {
    fn default() -> Self {
        Self {
            max_concurrent: default_submodule_max_concurrent(),
            max_failures: default_submodule_max_failures(),
            include_patterns: None,
            exclude_patterns: None,
        }
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

    #[test]
    fn lfs_config_defaults() {
        let config = LfsConfig::default();
        assert_eq!(config.retry_max_attempts, 3);
        assert_eq!(config.retry_initial_backoff_ms, 500);
        assert_eq!(config.retry_max_backoff_ms, 30_000);
        assert!((config.retry_backoff_multiplier - 2.0).abs() < f64::EPSILON);
        assert!(config.max_object_size.is_none());
        assert_eq!(config.request_timeout_secs, 300);
        assert_eq!(config.connect_timeout_secs, 30);
        assert_eq!(config.download_timeout_secs, 600);
    }

    #[test]
    fn parse_lfs_config() {
        let json = r#"{
            "lfs": {
                "retry_max_attempts": 5,
                "retry_initial_backoff_ms": 1000,
                "retry_max_backoff_ms": 60000,
                "retry_backoff_multiplier": 3.0,
                "max_object_size": 104857600
            }
        }"#;

        let config: Config = serde_json::from_str(json).unwrap();
        assert_eq!(config.lfs.retry_max_attempts, 5);
        assert_eq!(config.lfs.retry_initial_backoff_ms, 1000);
        assert_eq!(config.lfs.retry_max_backoff_ms, 60_000);
        assert!((config.lfs.retry_backoff_multiplier - 3.0).abs() < f64::EPSILON);
        assert_eq!(config.lfs.max_object_size, Some(104_857_600));
    }

    #[test]
    fn parse_lfs_config_partial() {
        let json = r#"{
            "lfs": {
                "retry_max_attempts": 10
            }
        }"#;

        let config: Config = serde_json::from_str(json).unwrap();
        assert_eq!(config.lfs.retry_max_attempts, 10);
        // Other fields should use defaults
        assert_eq!(config.lfs.retry_initial_backoff_ms, 500);
        assert_eq!(config.lfs.retry_max_backoff_ms, 30_000);
        assert!((config.lfs.retry_backoff_multiplier - 2.0).abs() < f64::EPSILON);
        assert!(config.lfs.max_object_size.is_none());
    }

    #[test]
    fn submodule_config_defaults() {
        let config = SubmoduleConfig::default();
        assert_eq!(config.max_concurrent, 4);
        assert_eq!(config.max_failures, 3);
        assert!(config.include_patterns.is_none());
        assert!(config.exclude_patterns.is_none());
    }

    #[test]
    fn parse_submodule_config() {
        let json = r#"{
            "submodules": {
                "max_concurrent": 8,
                "max_failures": 5,
                "include_patterns": ["lib/*", "deps/core"],
                "exclude_patterns": ["vendor/*"]
            }
        }"#;

        let config: Config = serde_json::from_str(json).unwrap();
        assert_eq!(config.submodules.max_concurrent, 8);
        assert_eq!(config.submodules.max_failures, 5);
        assert_eq!(
            config.submodules.include_patterns,
            Some(vec!["lib/*".to_string(), "deps/core".to_string()])
        );
        assert_eq!(
            config.submodules.exclude_patterns,
            Some(vec!["vendor/*".to_string()])
        );
    }

    #[test]
    fn parse_submodule_config_partial() {
        let json = r#"{
            "submodules": {
                "max_concurrent": 2
            }
        }"#;

        let config: Config = serde_json::from_str(json).unwrap();
        assert_eq!(config.submodules.max_concurrent, 2);
        // Other fields should use defaults
        assert_eq!(config.submodules.max_failures, 3);
        assert!(config.submodules.include_patterns.is_none());
        assert!(config.submodules.exclude_patterns.is_none());
    }

    #[test]
    fn parse_full_config_with_lfs_and_submodules() {
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
            "lfs": {
                "retry_max_attempts": 5,
                "max_object_size": 104857600
            },
            "submodules": {
                "exclude_patterns": ["vendor/*"]
            }
        }"#;

        let config: Config = serde_json::from_str(json).unwrap();
        assert!(config.validate().is_ok());
        assert_eq!(config.lfs.retry_max_attempts, 5);
        assert_eq!(config.lfs.max_object_size, Some(104_857_600));
        assert_eq!(
            config.submodules.exclude_patterns,
            Some(vec!["vendor/*".to_string()])
        );
    }
}
