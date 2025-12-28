//! Credential types for authentication.
//!
//! All sensitive data is wrapped in [`secrecy::SecretString`] which:
//! - Zeroises memory on drop
//! - Prevents accidental logging (no `Debug` impl that shows value)
//! - Requires explicit `.expose_secret()` to access

use std::path::PathBuf;

use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;

/// Authentication method for a remote.
///
/// This enum represents the different ways to authenticate with a Git remote.
/// Credentials are stored securely and never exposed in debug output.
#[derive(Debug)]
pub enum AuthMethod {
    /// Personal Access Token authentication (HTTPS).
    Pat(PatCredential),

    /// SSH key file authentication.
    SshKey(SshKeyCredential),

    /// SSH agent authentication.
    SshAgent(SshAgentCredential),
}

/// A credential entry matching a URL pattern.
///
/// This is the primary type used to match URLs to their authentication method.
#[derive(Debug)]
pub struct Credential {
    /// Human-readable name for this credential entry.
    name: String,

    /// URL pattern to match (supports glob patterns).
    /// Example: `https://github.com/*`
    url_pattern: String,

    /// The authentication method to use.
    auth: AuthMethod,
}

impl Credential {
    /// Creates a new credential entry.
    #[must_use]
    pub fn new(name: String, url_pattern: String, auth: AuthMethod) -> Self {
        Self {
            name,
            url_pattern,
            auth,
        }
    }

    /// Returns the name of this credential entry.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the URL pattern this credential matches.
    #[must_use]
    pub fn url_pattern(&self) -> &str {
        &self.url_pattern
    }

    /// Returns a reference to the authentication method.
    #[must_use]
    pub fn auth(&self) -> &AuthMethod {
        &self.auth
    }
}

/// Personal Access Token credential.
///
/// Used for HTTPS authentication with services like GitHub, GitLab, etc.
///
/// # Security
///
/// The token is stored as a [`SecretString`] and will be zeroised on drop.
/// The `Debug` implementation does NOT reveal the token value.
pub struct PatCredential {
    /// The actual token value, stored securely.
    token: SecretString,
}

impl PatCredential {
    /// Creates a new PAT credential.
    #[must_use]
    pub fn new(token: SecretString) -> Self {
        Self { token }
    }

    /// Exposes the token for use with git2.
    ///
    /// # Security
    ///
    /// Only call this when passing to git2's credential callbacks.
    /// Never log or store the returned value.
    #[must_use]
    pub fn expose_token(&self) -> &str {
        self.token.expose_secret()
    }
}

// Custom Debug that never reveals the token
impl std::fmt::Debug for PatCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PatCredential")
            .field("token", &"[REDACTED]")
            .finish()
    }
}

/// SSH key file credential.
///
/// Points to a private key file on disk, optionally with a passphrase.
///
/// # Security
///
/// The passphrase (if any) is stored as a [`SecretString`].
/// The key file itself is read only when needed by git2.
pub struct SshKeyCredential {
    /// Path to the private key file.
    key_path: PathBuf,

    /// Optional passphrase for encrypted keys.
    passphrase: Option<SecretString>,
}

impl SshKeyCredential {
    /// Creates a new SSH key credential.
    #[must_use]
    pub fn new(key_path: PathBuf, passphrase: Option<SecretString>) -> Self {
        Self {
            key_path,
            passphrase,
        }
    }

    /// Returns the path to the SSH key file.
    #[must_use]
    pub fn key_path(&self) -> &PathBuf {
        &self.key_path
    }

    /// Exposes the passphrase for use with git2.
    ///
    /// Returns `None` if no passphrase was configured.
    ///
    /// # Security
    ///
    /// Only call this when passing to git2's credential callbacks.
    #[must_use]
    pub fn expose_passphrase(&self) -> Option<&str> {
        self.passphrase.as_ref().map(|p| p.expose_secret())
    }
}

// Custom Debug that never reveals the passphrase
impl std::fmt::Debug for SshKeyCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SshKeyCredential")
            .field("key_path", &self.key_path)
            .field(
                "passphrase",
                &self.passphrase.as_ref().map(|_| "[REDACTED]"),
            )
            .finish()
    }
}

/// SSH agent credential.
///
/// Uses the system's SSH agent for authentication.
/// No credentials are stored locally — the agent handles everything.
#[derive(Debug, Clone)]
pub struct SshAgentCredential {
    /// Optional specific identity to use from the agent.
    /// If `None`, the agent will try all available identities.
    identity_file: Option<PathBuf>,
}

impl SshAgentCredential {
    /// Creates a new SSH agent credential.
    #[must_use]
    pub fn new(identity_file: Option<PathBuf>) -> Self {
        Self { identity_file }
    }

    /// Returns the optional identity file hint.
    #[must_use]
    pub fn identity_file(&self) -> Option<&PathBuf> {
        self.identity_file.as_ref()
    }
}

impl Default for SshAgentCredential {
    fn default() -> Self {
        Self::new(None)
    }
}

/// Helper for deserialising secrets from config.
///
/// This ensures secrets are immediately wrapped in `SecretString`
/// during deserialisation, never existing as plain `String` in memory.
pub(crate) fn deserialize_secret<'de, D>(deserializer: D) -> Result<SecretString, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    Ok(SecretString::from(s))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pat_debug_does_not_leak_token() {
        let cred = PatCredential::new(SecretString::from("ghp_supersecrettoken123"));
        let debug_output = format!("{cred:?}");

        assert!(!debug_output.contains("ghp_"));
        assert!(!debug_output.contains("supersecret"));
        assert!(debug_output.contains("REDACTED"));
    }

    #[test]
    fn ssh_key_debug_does_not_leak_passphrase() {
        let cred = SshKeyCredential::new(
            PathBuf::from("/home/user/.ssh/id_ed25519"),
            Some(SecretString::from("mysuperpassphrase")),
        );
        let debug_output = format!("{cred:?}");

        assert!(!debug_output.contains("mysuperpassphrase"));
        assert!(debug_output.contains("REDACTED"));
        assert!(debug_output.contains("id_ed25519")); // path is OK to show
    }

    #[test]
    fn pat_expose_token_works() {
        let token = "ghp_test123";
        let cred = PatCredential::new(SecretString::from(token));

        assert_eq!(cred.expose_token(), token);
    }

    #[test]
    fn ssh_key_expose_passphrase_works() {
        let passphrase = "secret123";
        let cred = SshKeyCredential::new(
            PathBuf::from("/path/to/key"),
            Some(SecretString::from(passphrase)),
        );

        assert_eq!(cred.expose_passphrase(), Some(passphrase));
    }

    #[test]
    fn ssh_key_no_passphrase() {
        let cred = SshKeyCredential::new(PathBuf::from("/path/to/key"), None);

        assert_eq!(cred.expose_passphrase(), None);
    }

    #[test]
    fn ssh_agent_default() {
        let cred = SshAgentCredential::default();

        assert!(cred.identity_file().is_none());
    }
}
