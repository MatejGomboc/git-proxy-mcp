//! URL pattern matching for credential selection.
//!
//! This module provides the [`UrlPattern`] and [`CredentialStore`] types
//! for matching Git URLs to their appropriate credentials.
//!
//! # Pattern Syntax
//!
//! Patterns use glob-style wildcards:
//!
//! - `*` matches any sequence of characters (including `/`)
//! - `?` matches any single character
//! - `[abc]` matches any character in the set
//! - `[!abc]` matches any character not in the set
//!
//! # Examples
//!
//! ```text
//! https://github.com/*         matches https://github.com/user/repo
//! git@github.com:*             matches git@github.com:user/repo.git
//! https://git.company.com/*    matches https://git.company.com/team/project
//! ```
//!
//! # Matching Order
//!
//! Credentials are matched in the order they appear in the configuration.
//! The first matching credential is returned.

use glob::{MatchOptions, Pattern, PatternError};

use crate::auth::Credential;
use crate::error::AuthError;

/// A compiled URL pattern for matching Git URLs.
///
/// Patterns are compiled once and can be reused for multiple URL matches.
/// The original pattern string is preserved for debugging and error messages.
#[derive(Debug)]
pub struct UrlPattern {
    /// The original pattern string (for Debug output and error messages).
    original: String,

    /// The compiled glob pattern.
    pattern: Pattern,
}

impl UrlPattern {
    /// Creates a new URL pattern from a pattern string.
    ///
    /// # Errors
    ///
    /// Returns [`AuthError::InvalidUrlPattern`] if the pattern has invalid
    /// glob syntax.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// let pattern = UrlPattern::new("https://github.com/*")?;
    /// assert!(pattern.matches("https://github.com/user/repo"));
    /// ```
    pub fn new(pattern: &str) -> Result<Self, AuthError> {
        let compiled =
            Pattern::new(pattern).map_err(|e: PatternError| AuthError::InvalidUrlPattern {
                pattern: pattern.to_string(),
                reason: e.msg.to_string(),
            })?;

        Ok(Self {
            original: pattern.to_string(),
            pattern: compiled,
        })
    }

    /// Returns the original pattern string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.original
    }

    /// Checks if the given URL matches this pattern.
    ///
    /// The match is case-sensitive and allows `*` to match path separators.
    #[must_use]
    pub fn matches(&self, url: &str) -> bool {
        let options = MatchOptions {
            // Allow * to match / in URLs (e.g., https://github.com/* matches /user/repo)
            require_literal_separator: false,
            // Case-sensitive matching (URLs are case-sensitive)
            case_sensitive: true,
            // Don't require literal leading dots
            require_literal_leading_dot: false,
        };

        self.pattern.matches_with(url, options)
    }
}

/// A store of credentials with URL pattern matching capabilities.
///
/// Credentials are matched in order — the first matching credential is returned.
/// This allows users to configure more specific patterns before general ones.
///
/// # Example Configuration Order
///
/// ```json
/// [
///   { "url_pattern": "https://github.com/mycompany/*", "auth": "..." },
///   { "url_pattern": "https://github.com/*", "auth": "..." }
/// ]
/// ```
///
/// In this example, company repos use the first credential, while other
/// GitHub repos fall through to the second.
pub struct CredentialStore {
    /// Credentials with their compiled patterns.
    entries: Vec<CredentialEntry>,
}

/// A credential paired with its compiled URL pattern.
struct CredentialEntry {
    /// The compiled URL pattern.
    pattern: UrlPattern,

    /// The credential to use when the pattern matches.
    credential: Credential,
}

impl CredentialStore {
    /// Creates a new credential store from a list of credentials.
    ///
    /// Patterns are compiled during construction. Invalid patterns are
    /// skipped with a warning (in a real implementation, this might
    /// return an error instead).
    ///
    /// # Errors
    ///
    /// Returns an error if any credential has an invalid URL pattern.
    pub fn new(credentials: Vec<Credential>) -> Result<Self, AuthError> {
        let mut entries = Vec::with_capacity(credentials.len());

        for credential in credentials {
            let pattern = UrlPattern::new(credential.url_pattern())?;
            entries.push(CredentialEntry {
                pattern,
                credential,
            });
        }

        Ok(Self { entries })
    }

    /// Finds the first credential matching the given URL.
    ///
    /// Credentials are checked in order, and the first match is returned.
    ///
    /// # Errors
    ///
    /// Returns [`AuthError::NoMatchingCredential`] if no credential matches
    /// the URL.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// let cred = store.find_credential("https://github.com/user/repo")?;
    /// println!("Using credential: {}", cred.name());
    /// ```
    pub fn find_credential(&self, url: &str) -> Result<&Credential, AuthError> {
        for entry in &self.entries {
            if entry.pattern.matches(url) {
                return Ok(&entry.credential);
            }
        }

        Err(AuthError::NoMatchingCredential {
            url: url.to_string(),
        })
    }

    /// Returns the number of credentials in the store.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns `true` if the store contains no credentials.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl std::fmt::Debug for CredentialStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CredentialStore")
            .field("count", &self.entries.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use secrecy::SecretString;

    use super::*;
    use crate::auth::{AuthMethod, PatCredential};

    /// Helper to create a test credential.
    fn test_credential(name: &str, url_pattern: &str) -> Credential {
        Credential::new(
            name.to_string(),
            url_pattern.to_string(),
            AuthMethod::Pat(PatCredential::new(SecretString::from("test_token"))),
        )
    }

    // =========================================================================
    // UrlPattern tests
    // =========================================================================

    #[test]
    fn pattern_compiles_valid_pattern() {
        let pattern = UrlPattern::new("https://github.com/*");
        assert!(pattern.is_ok());
    }

    #[test]
    fn pattern_rejects_invalid_pattern() {
        // Unclosed bracket is invalid glob syntax
        let pattern = UrlPattern::new("https://github.com/[unclosed");
        assert!(pattern.is_err());

        if let Err(AuthError::InvalidUrlPattern { pattern, .. }) = pattern {
            assert!(pattern.contains("[unclosed"));
        } else {
            panic!("Expected InvalidUrlPattern error");
        }
    }

    #[test]
    fn pattern_matches_https_url() {
        let pattern = UrlPattern::new("https://github.com/*").unwrap();

        assert!(pattern.matches("https://github.com/user/repo"));
        assert!(pattern.matches("https://github.com/user/repo.git"));
        assert!(pattern.matches("https://github.com/org/project/subdir"));
    }

    #[test]
    fn pattern_rejects_non_matching_https_url() {
        let pattern = UrlPattern::new("https://github.com/*").unwrap();

        assert!(!pattern.matches("https://gitlab.com/user/repo"));
        assert!(!pattern.matches("http://github.com/user/repo")); // http vs https
    }

    #[test]
    fn pattern_matches_ssh_url() {
        let pattern = UrlPattern::new("git@github.com:*").unwrap();

        assert!(pattern.matches("git@github.com:user/repo"));
        assert!(pattern.matches("git@github.com:user/repo.git"));
        assert!(pattern.matches("git@github.com:org/project"));
    }

    #[test]
    fn pattern_rejects_non_matching_ssh_url() {
        let pattern = UrlPattern::new("git@github.com:*").unwrap();

        assert!(!pattern.matches("git@gitlab.com:user/repo"));
        assert!(!pattern.matches("https://github.com/user/repo"));
    }

    #[test]
    fn pattern_is_case_sensitive() {
        let pattern = UrlPattern::new("https://GitHub.com/*").unwrap();

        assert!(pattern.matches("https://GitHub.com/user/repo"));
        assert!(!pattern.matches("https://github.com/user/repo")); // lowercase
    }

    #[test]
    fn pattern_preserves_original_string() {
        let pattern = UrlPattern::new("https://example.com/*").unwrap();
        assert_eq!(pattern.as_str(), "https://example.com/*");
    }

    // =========================================================================
    // CredentialStore tests
    // =========================================================================

    #[test]
    fn store_finds_matching_credential() {
        let credentials = vec![
            test_credential("github", "https://github.com/*"),
            test_credential("gitlab", "https://gitlab.com/*"),
        ];

        let store = CredentialStore::new(credentials).unwrap();

        let found = store
            .find_credential("https://github.com/user/repo")
            .unwrap();
        assert_eq!(found.name(), "github");

        let found = store
            .find_credential("https://gitlab.com/user/repo")
            .unwrap();
        assert_eq!(found.name(), "gitlab");
    }

    #[test]
    fn store_returns_first_match() {
        // More specific pattern first
        let credentials = vec![
            test_credential("company", "https://github.com/mycompany/*"),
            test_credential("general", "https://github.com/*"),
        ];

        let store = CredentialStore::new(credentials).unwrap();

        // Company URL matches first credential
        let found = store
            .find_credential("https://github.com/mycompany/repo")
            .unwrap();
        assert_eq!(found.name(), "company");

        // Other GitHub URLs match second credential
        let found = store
            .find_credential("https://github.com/other/repo")
            .unwrap();
        assert_eq!(found.name(), "general");
    }

    #[test]
    fn store_returns_error_when_no_match() {
        let credentials = vec![test_credential("github", "https://github.com/*")];

        let store = CredentialStore::new(credentials).unwrap();

        let result = store.find_credential("https://gitlab.com/user/repo");
        assert!(result.is_err());

        if let Err(AuthError::NoMatchingCredential { url }) = result {
            assert_eq!(url, "https://gitlab.com/user/repo");
        } else {
            panic!("Expected NoMatchingCredential error");
        }
    }

    #[test]
    fn store_rejects_invalid_pattern() {
        let credentials = vec![test_credential("bad", "https://example.com/[invalid")];

        let result = CredentialStore::new(credentials);
        assert!(result.is_err());
    }

    #[test]
    fn store_reports_correct_count() {
        let credentials = vec![
            test_credential("one", "https://one.com/*"),
            test_credential("two", "https://two.com/*"),
            test_credential("three", "https://three.com/*"),
        ];

        let store = CredentialStore::new(credentials).unwrap();
        assert_eq!(store.len(), 3);
        assert!(!store.is_empty());
    }

    #[test]
    fn store_empty_is_empty() {
        let store = CredentialStore::new(vec![]).unwrap();
        assert!(store.is_empty());
        assert_eq!(store.len(), 0);
    }
}
