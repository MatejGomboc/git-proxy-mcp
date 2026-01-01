//! Output sanitisation to prevent credential leakage.
//!
//! This module scans Git command output for patterns that might indicate
//! credential leakage and redacts them.
//!
//! # Security
//!
//! Even with careful credential injection, credentials can sometimes appear
//! in output:
//!
//! - Error messages that include the full URL (with embedded credentials)
//! - Debug output from Git or underlying libraries
//! - Verbose mode output
//!
//! This sanitiser acts as a last line of defence.

use std::borrow::Cow;

/// Patterns that indicate potential credential leakage.
///
/// These patterns are checked against output and redacted if found.
const CREDENTIAL_PATTERNS: &[&str] = &[
    // GitHub PAT prefixes
    "ghp_", // Personal access token
    "gho_", // OAuth token
    "ghu_", // User-to-server token
    "ghs_", // Server-to-server token
    "ghr_", // Refresh token
    // GitLab tokens
    "glpat-", // Personal access token
    "gloas-", // OAuth token
    "gldt-",  // Deploy token
    "glrt-",  // Runner token
    "glcbt-", // CI/CD job token
    // Bitbucket tokens
    "ATBB", // App password prefix
    // Azure DevOps
    "azure://",
    // Generic patterns
    "x-access-token",
    "x-oauth-basic",
    "Authorization:",
    "Bearer ",
    // SSH key patterns (should never appear, but just in case)
    "-----BEGIN",
    "-----END",
    "PRIVATE KEY",
];

/// URL patterns that might contain embedded credentials.
///
/// Format: `https://username:password@host/path`
const URL_CREDENTIAL_PATTERN: &str = "://";

/// Sanitises Git command output to prevent credential leakage.
#[derive(Debug, Default)]
pub struct OutputSanitiser {
    /// Additional patterns to check (from configuration).
    custom_patterns: Vec<String>,
}

impl OutputSanitiser {
    /// Creates a new output sanitiser.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a custom pattern to check.
    pub fn add_pattern(&mut self, pattern: impl Into<String>) {
        self.custom_patterns.push(pattern.into());
    }

    /// Sanitises a string, redacting any detected credentials.
    ///
    /// Returns a `Cow::Borrowed` if no changes were made, or `Cow::Owned`
    /// if the string was modified.
    #[must_use]
    pub fn sanitise<'a>(&self, input: &'a str) -> Cow<'a, str> {
        let mut result = Cow::Borrowed(input);

        // Check built-in patterns
        for pattern in CREDENTIAL_PATTERNS {
            if result.contains(pattern) {
                result = Cow::Owned(Self::redact_pattern(&result, pattern));
            }
        }

        // Check custom patterns
        for pattern in &self.custom_patterns {
            if result.contains(pattern.as_str()) {
                result = Cow::Owned(Self::redact_pattern(&result, pattern));
            }
        }

        // Check for URLs with embedded credentials
        result = Self::sanitise_urls(result);

        result
    }

    /// Redacts a pattern from a string.
    fn redact_pattern(input: &str, pattern: &str) -> String {
        // Find the pattern and redact the rest of the "word" (until whitespace or end)
        let mut result = String::with_capacity(input.len());
        let mut remaining = input;

        while let Some(pos) = remaining.find(pattern) {
            // Add everything before the pattern
            result.push_str(&remaining[..pos]);

            // Find the end of the credential (next whitespace, quote, or end)
            let after_pattern = &remaining[pos..];
            let end_pos = after_pattern
                .find(|c: char| c.is_whitespace() || c == '"' || c == '\'' || c == '>' || c == '<')
                .unwrap_or(after_pattern.len());

            // Add redaction
            result.push_str("[REDACTED]");

            // Move past the redacted portion
            remaining = &remaining[pos + end_pos..];
        }

        // Add any remaining text
        result.push_str(remaining);
        result
    }

    /// Sanitises URLs that might contain embedded credentials.
    ///
    /// Looks for patterns like `https://user:pass@host/` and redacts the credentials.
    fn sanitise_urls(input: Cow<'_, str>) -> Cow<'_, str> {
        if !input.contains(URL_CREDENTIAL_PATTERN) {
            return input;
        }

        // Look for URLs with credentials: scheme://user:pass@host
        // We need to find :// followed by something@
        let mut result = String::new();
        let mut last_end = 0;
        let input_str = input.as_ref();
        let bytes = input_str.as_bytes();

        let mut i = 0;
        while i < bytes.len().saturating_sub(3) {
            // Look for ://
            if bytes[i] == b':'
                && bytes.get(i + 1) == Some(&b'/')
                && bytes.get(i + 2) == Some(&b'/')
            {
                // Found ://, now look for @ before the next /
                let start_of_auth = i + 3;
                let mut at_pos = None;
                let mut slash_pos = None;

                for (offset, &byte) in bytes[start_of_auth..].iter().enumerate() {
                    if byte == b'@' && at_pos.is_none() {
                        at_pos = Some(start_of_auth + offset);
                    } else if byte == b'/' {
                        slash_pos = Some(start_of_auth + offset);
                        break;
                    }
                }

                // If we found @ before / (or end), there might be credentials
                if let Some(at) = at_pos {
                    let auth_end = slash_pos.unwrap_or(bytes.len());
                    if at < auth_end {
                        // Check if there's a : in the auth section (user:pass)
                        let auth_section = &input_str[start_of_auth..at];
                        if auth_section.contains(':') {
                            // Found credentials, redact them
                            result.push_str(&input_str[last_end..=i + 2]); // include ://
                            result.push_str("[REDACTED]@");
                            last_end = at + 1;
                            i = at + 1;
                            continue;
                        }
                    }
                }
            }
            i += 1;
        }

        if last_end == 0 {
            // No changes made
            input
        } else {
            result.push_str(&input_str[last_end..]);
            Cow::Owned(result)
        }
    }

    /// Checks if a string contains any credential patterns.
    ///
    /// This is a quick check without doing the full sanitisation.
    #[must_use]
    pub fn contains_credentials(&self, input: &str) -> bool {
        // Check built-in patterns
        for pattern in CREDENTIAL_PATTERNS {
            if input.contains(pattern) {
                return true;
            }
        }

        // Check custom patterns
        for pattern in &self.custom_patterns {
            if input.contains(pattern.as_str()) {
                return true;
            }
        }

        // Check for URL credentials
        if input.contains(URL_CREDENTIAL_PATTERN) {
            // Quick check for @ after ://
            for (i, _) in input.match_indices("://") {
                let after = &input[i + 3..];
                if let Some(at_pos) = after.find('@') {
                    // Check if @ comes before next /
                    let slash_pos = after.find('/').unwrap_or(after.len());
                    if at_pos < slash_pos && after[..at_pos].contains(':') {
                        return true;
                    }
                }
            }
        }

        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitise_github_pat() {
        let sanitiser = OutputSanitiser::new();
        let input = "Authentication failed for token ghp_1234567890abcdef";
        let output = sanitiser.sanitise(input);

        assert!(!output.contains("ghp_"));
        assert!(output.contains("[REDACTED]"));
    }

    #[test]
    fn sanitise_gitlab_pat() {
        let sanitiser = OutputSanitiser::new();
        let input = "Using token: glpat-abcdefghijk";
        let output = sanitiser.sanitise(input);

        assert!(!output.contains("glpat-"));
        assert!(output.contains("[REDACTED]"));
    }

    #[test]
    fn sanitise_url_with_credentials() {
        let sanitiser = OutputSanitiser::new();
        let input = "Cloning from https://user:secretpass@github.com/repo.git";
        let output = sanitiser.sanitise(input);

        assert!(!output.contains("secretpass"));
        assert!(!output.contains("user:"));
        assert!(output.contains("[REDACTED]@"));
        assert!(output.contains("github.com"));
    }

    #[test]
    fn preserve_url_without_credentials() {
        let sanitiser = OutputSanitiser::new();
        let input = "Cloning from https://github.com/user/repo.git";
        let output = sanitiser.sanitise(input);

        assert_eq!(output.as_ref(), input);
    }

    #[test]
    fn sanitise_authorization_header() {
        let sanitiser = OutputSanitiser::new();
        let input = "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJ1c2VyIn0";
        let output = sanitiser.sanitise(input);

        assert!(output.contains("[REDACTED]"));
    }

    #[test]
    fn sanitise_ssh_key() {
        let sanitiser = OutputSanitiser::new();
        let input = "Key: -----BEGIN RSA PRIVATE KEY-----";
        let output = sanitiser.sanitise(input);

        assert!(!output.contains("-----BEGIN"));
        assert!(output.contains("[REDACTED]"));
    }

    #[test]
    fn no_change_for_safe_output() {
        let sanitiser = OutputSanitiser::new();
        let input = "Cloning into 'repo'...\nremote: Counting objects: 100";
        let output = sanitiser.sanitise(input);

        // Should be borrowed (no allocation)
        assert!(matches!(output, Cow::Borrowed(_)));
        assert_eq!(output.as_ref(), input);
    }

    #[test]
    fn contains_credentials_detects_pat() {
        let sanitiser = OutputSanitiser::new();
        assert!(sanitiser.contains_credentials("token: ghp_secret123"));
        assert!(sanitiser.contains_credentials("glpat-secret123"));
    }

    #[test]
    fn contains_credentials_detects_url_creds() {
        let sanitiser = OutputSanitiser::new();
        assert!(sanitiser.contains_credentials("https://user:pass@host.com/"));
        assert!(!sanitiser.contains_credentials("https://host.com/user/repo"));
    }

    #[test]
    fn custom_pattern() {
        let mut sanitiser = OutputSanitiser::new();
        sanitiser.add_pattern("MY_SECRET_");

        let input = "Using MY_SECRET_abc123 for auth";
        let output = sanitiser.sanitise(input);

        assert!(!output.contains("MY_SECRET_"));
        assert!(output.contains("[REDACTED]"));
    }

    #[test]
    fn multiple_credentials_in_one_string() {
        let sanitiser = OutputSanitiser::new();
        let input = "Tokens: ghp_first123 and glpat-second456";
        let output = sanitiser.sanitise(input);

        assert!(!output.contains("ghp_"));
        assert!(!output.contains("glpat-"));
        assert_eq!(output.matches("[REDACTED]").count(), 2);
    }
}
