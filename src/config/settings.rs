//! Configuration structures for deserialisation.
//!
//! These structures map directly to the JSON configuration file format.
//! Sensitive fields are deserialised directly into `SecretString` types.

use std::path::PathBuf;

use secrecy::SecretString;
use serde::Deserialize;

use crate::auth::{AuthMethod, Credential, PatCredential, SshAgentCredential, SshKeyCredential};
use crate::config::expand_tilde;
use crate::error::ConfigError;

/// Root configuration structure.
///
/// This is the top-level structure that matches the JSON config file.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Optional JSON schema reference (ignored during parsing).
    #[serde(rename = "$schema", default)]
    _schema: Option<String>,

    /// Optional comment field (ignored during parsing).
    #[serde(rename = "_comment", default)]
    _comment: Option<String>,

    /// List of remote configurations with authentication.
    pub remotes: Vec<RemoteConfig>,

    /// Identity to use for AI-generated commits.
    #[serde(default)]
    pub ai_identity: Option<AiIdentity>,

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
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.remotes.is_empty() {
            return Err(ConfigError::ValidationError {
                message: "at least one remote must be configured".to_string(),
            });
        }

        for remote in &self.remotes {
            remote.validate()?;
        }

        Ok(())
    }

    /// Converts the configuration into a list of credentials.
    ///
    /// This transforms the parsed configuration into the secure credential
    /// types that will be used at runtime.
    #[must_use]
    pub fn into_credentials(self) -> Vec<Credential> {
        self.remotes
            .into_iter()
            .map(|remote| {
                let auth = remote.auth.into_auth_method();
                Credential::new(remote.name, remote.url_pattern, auth)
            })
            .collect()
    }
}

/// Configuration for a single remote.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteConfig {
    /// Human-readable name for this remote.
    pub name: String,

    /// URL pattern to match (supports glob patterns).
    pub url_pattern: String,

    /// Authentication configuration.
    pub auth: AuthConfig,
}

impl RemoteConfig {
    /// Validates the remote configuration.
    fn validate(&self) -> Result<(), ConfigError> {
        if self.name.is_empty() {
            return Err(ConfigError::ValidationError {
                message: "remote name cannot be empty".to_string(),
            });
        }

        if self.url_pattern.is_empty() {
            return Err(ConfigError::ValidationError {
                message: format!("url_pattern cannot be empty for remote '{}'", self.name),
            });
        }

        self.auth.validate(&self.name)?;

        Ok(())
    }
}

/// Authentication configuration.
///
/// Uses serde's tag feature to deserialise the correct variant based on
/// the `type` field in the JSON.
#[derive(Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum AuthConfig {
    /// Personal Access Token authentication.
    #[serde(rename = "pat")]
    Pat {
        /// The token value (stored securely).
        #[serde(deserialize_with = "deserialize_secret")]
        token: SecretString,
    },

    /// SSH key file authentication.
    #[serde(rename = "ssh_key")]
    SshKey {
        /// Path to the private key file.
        key_path: String,

        /// Optional passphrase for encrypted keys.
        #[serde(default, deserialize_with = "deserialize_option_secret")]
        passphrase: Option<SecretString>,
    },

    /// SSH agent authentication.
    #[serde(rename = "ssh_agent")]
    SshAgent {
        /// Optional specific identity file to use.
        #[serde(default)]
        identity_file: Option<String>,
    },
}

impl AuthConfig {
    /// Validates the authentication configuration.
    fn validate(&self, remote_name: &str) -> Result<(), ConfigError> {
        match self {
            Self::Pat { .. } => {
                // Token presence is enforced by serde (not Option)
                Ok(())
            }
            Self::SshKey { key_path, .. } => {
                if key_path.is_empty() {
                    return Err(ConfigError::ValidationError {
                        message: format!(
                            "key_path cannot be empty for SSH key auth on remote '{remote_name}'"
                        ),
                    });
                }
                Ok(())
            }
            Self::SshAgent { .. } => {
                // SSH agent has no required fields
                Ok(())
            }
        }
    }

    /// Converts into the secure `AuthMethod` type.
    fn into_auth_method(self) -> AuthMethod {
        match self {
            Self::Pat { token } => AuthMethod::Pat(PatCredential::new(token)),
            Self::SshKey {
                key_path,
                passphrase,
            } => {
                let expanded_path = expand_tilde(&key_path);
                AuthMethod::SshKey(SshKeyCredential::new(expanded_path, passphrase))
            }
            Self::SshAgent { identity_file } => {
                let expanded_path = identity_file.map(|p| expand_tilde(&p));
                AuthMethod::SshAgent(SshAgentCredential::new(expanded_path))
            }
        }
    }
}

// Custom Debug that never reveals secrets
impl std::fmt::Debug for AuthConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pat { .. } => f.debug_struct("Pat").field("token", &"[REDACTED]").finish(),
            Self::SshKey {
                key_path,
                passphrase,
            } => f
                .debug_struct("SshKey")
                .field("key_path", key_path)
                .field("passphrase", &passphrase.as_ref().map(|_| "[REDACTED]"))
                .finish(),
            Self::SshAgent { identity_file } => f
                .debug_struct("SshAgent")
                .field("identity_file", identity_file)
                .finish(),
        }
    }
}

/// Identity configuration for AI-generated commits.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AiIdentity {
    /// Name to use in commit author field.
    pub name: String,

    /// Email to use in commit author field.
    pub email: String,
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

/// Deserialises a string into a `SecretString`.
fn deserialize_secret<'de, D>(deserializer: D) -> Result<SecretString, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    Ok(SecretString::from(s))
}

/// Deserialises an optional string into an `Option<SecretString>`.
fn deserialize_option_secret<'de, D>(deserializer: D) -> Result<Option<SecretString>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let opt: Option<String> = Option::deserialize(deserializer)?;
    Ok(opt.map(SecretString::from))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal_config() {
        let json = r#"{
            "remotes": [
                {
                    "name": "github",
                    "url_pattern": "https://github.com/*",
                    "auth": {
                        "type": "pat",
                        "token": "ghp_test123"
                    }
                }
            ]
        }"#;

        let config: Config = serde_json::from_str(json).unwrap();
        assert_eq!(config.remotes.len(), 1);
        assert_eq!(config.remotes[0].name, "github");
    }

    #[test]
    fn parse_full_config() {
        let json = r#"{
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "_comment": "Test config",
            "remotes": [
                {
                    "name": "github",
                    "url_pattern": "https://github.com/*",
                    "auth": {
                        "type": "pat",
                        "token": "ghp_test123"
                    }
                },
                {
                    "name": "company",
                    "url_pattern": "https://git.company.com/*",
                    "auth": {
                        "type": "ssh_key",
                        "key_path": "~/.ssh/id_ed25519",
                        "passphrase": "secret"
                    }
                },
                {
                    "name": "bitbucket",
                    "url_pattern": "git@bitbucket.org:*",
                    "auth": {
                        "type": "ssh_agent"
                    }
                }
            ],
            "ai_identity": {
                "name": "AI Assistant",
                "email": "ai@example.com"
            },
            "security": {
                "allow_force_push": false,
                "protected_branches": ["main", "master"]
            },
            "logging": {
                "level": "debug"
            }
        }"#;

        let config: Config = serde_json::from_str(json).unwrap();
        assert!(config.validate().is_ok());
        assert_eq!(config.remotes.len(), 3);
        assert!(config.ai_identity.is_some());
        assert!(!config.security.allow_force_push);
        assert_eq!(config.logging.level, "debug");
    }

    #[test]
    fn parse_ssh_key_without_passphrase() {
        let json = r#"{
            "remotes": [
                {
                    "name": "test",
                    "url_pattern": "https://example.com/*",
                    "auth": {
                        "type": "ssh_key",
                        "key_path": "~/.ssh/id_rsa"
                    }
                }
            ]
        }"#;

        let config: Config = serde_json::from_str(json).unwrap();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn validate_empty_remotes_fails() {
        let json = r#"{ "remotes": [] }"#;

        let config: Config = serde_json::from_str(json).unwrap();
        let result = config.validate();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("at least one remote"));
    }

    #[test]
    fn validate_empty_name_fails() {
        let json = r#"{
            "remotes": [
                {
                    "name": "",
                    "url_pattern": "https://github.com/*",
                    "auth": { "type": "ssh_agent" }
                }
            ]
        }"#;

        let config: Config = serde_json::from_str(json).unwrap();
        let result = config.validate();
        assert!(result.is_err());
    }

    #[test]
    fn auth_debug_does_not_leak_token() {
        let json = r#"{
            "type": "pat",
            "token": "ghp_supersecrettoken"
        }"#;

        let auth: AuthConfig = serde_json::from_str(json).unwrap();
        let debug_output = format!("{auth:?}");

        assert!(!debug_output.contains("ghp_"));
        assert!(!debug_output.contains("supersecret"));
        assert!(debug_output.contains("REDACTED"));
    }

    #[test]
    fn into_credentials_converts_correctly() {
        let json = r#"{
            "remotes": [
                {
                    "name": "github",
                    "url_pattern": "https://github.com/*",
                    "auth": {
                        "type": "pat",
                        "token": "ghp_test"
                    }
                }
            ]
        }"#;

        let config: Config = serde_json::from_str(json).unwrap();
        let credentials = config.into_credentials();

        assert_eq!(credentials.len(), 1);
        assert_eq!(credentials[0].name(), "github");
        assert_eq!(credentials[0].url_pattern(), "https://github.com/*");
    }
}
