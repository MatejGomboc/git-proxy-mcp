//! Credential types for authentication.
//!
//! All sensitive data is wrapped in [`secrecy::SecretString`] which:
//! - Zeroises memory on drop
//! - Prevents accidental logging (no `Debug` impl that shows value)
//! - Requires explicit `.expose_secret()` to access

use std::path::PathBuf;

use secrecy::{ExposeSecret, SecretString};

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
    pub const fn new(name: String, url_pattern: String, auth: AuthMethod) -> Self {
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
    pub const fn auth(&self) -> &AuthMethod {
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
    pub const fn new(token: SecretString) -> Self {
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

/// SSH key file credential (passphrase-less keys only).
///
/// Points to a private key file on disk. For passphrase-protected keys,
/// use [`SshAgentCredential`] instead — add the key to ssh-agent with `ssh-add`.
///
/// # Security
///
/// This type intentionally does not support passphrases. Storing passphrases
/// in config files is a security anti-pattern. Use ssh-agent for secure
/// passphrase handling.
#[derive(Debug)]
pub struct SshKeyCredential {
    /// Path to the private key file.
    key_path: PathBuf,
}

impl SshKeyCredential {
    /// Creates a new SSH key credential.
    #[must_use]
    pub const fn new(key_path: PathBuf) -> Self {
        Self { key_path }
    }

    /// Returns the path to the SSH key file.
    #[must_use]
    pub const fn key_path(&self) -> &PathBuf {
        &self.key_path
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
    pub const fn new(identity_file: Option<PathBuf>) -> Self {
        Self { identity_file }
    }

    /// Returns the optional identity file hint.
    #[must_use]
    pub const fn identity_file(&self) -> Option<&PathBuf> {
        self.identity_file.as_ref()
    }
}

impl Default for SshAgentCredential {
    fn default() -> Self {
        Self::new(None)
    }
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
    fn ssh_key_debug_shows_path() {
        let cred = SshKeyCredential::new(PathBuf::from("/home/user/.ssh/id_ed25519"));
        let debug_output = format!("{cred:?}");

        // Path is safe to show in debug output
        assert!(debug_output.contains("id_ed25519"));
    }

    #[test]
    fn pat_expose_token_works() {
        let token = "ghp_test123";
        let cred = PatCredential::new(SecretString::from(token));

        assert_eq!(cred.expose_token(), token);
    }

    #[test]
    fn ssh_key_stores_path() {
        let path = PathBuf::from("/path/to/key");
        let cred = SshKeyCredential::new(path.clone());

        assert_eq!(cred.key_path(), &path);
    }

    #[test]
    fn ssh_agent_default() {
        let cred = SshAgentCredential::default();

        assert!(cred.identity_file().is_none());
    }
}
