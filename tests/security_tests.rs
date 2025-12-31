//! Security tests for credential leak detection.
//!
//! These tests verify that credentials are never leaked in:
//! - Command output (stdout/stderr)
//! - Error messages
//! - Log entries
//! - MCP responses

use git_proxy_mcp::git::sanitiser::OutputSanitiser;
use git_proxy_mcp::security::audit::AuditEvent;
use git_proxy_mcp::security::guards::{BranchGuard, PushGuard, RepoFilter, SecurityGuard};

// =============================================================================
// Output Sanitisation Tests
// =============================================================================

#[test]
fn test_github_pat_redaction() {
    let sanitiser = OutputSanitiser::new();

    let test_cases = [
        // Personal access tokens
        ("Token: ghp_1234567890abcdefghijklmnop", "[REDACTED]"),
        // OAuth tokens
        ("Auth: gho_abcdef123456", "[REDACTED]"),
        // User-to-server tokens
        ("Token: ghu_xyz789", "[REDACTED]"),
        // Server-to-server tokens
        ("Token: ghs_servertoken", "[REDACTED]"),
        // Refresh tokens
        ("Refresh: ghr_refreshtoken", "[REDACTED]"),
    ];

    for (input, expected_marker) in test_cases {
        let output = sanitiser.sanitise(input);
        assert!(
            output.contains(expected_marker),
            "Output should contain redaction marker for input: {input}"
        );
        assert!(
            !output.contains("ghp_")
                && !output.contains("gho_")
                && !output.contains("ghu_")
                && !output.contains("ghs_")
                && !output.contains("ghr_"),
            "Output should not contain token prefixes: {output}"
        );
    }
}

#[test]
fn test_gitlab_token_redaction() {
    let sanitiser = OutputSanitiser::new();

    let test_cases = [
        "Token: glpat-abcdefghij1234567890",
        "OAuth: gloas-oauthtoken123",
        "Deploy: gldt-deploytoken456",
        "Runner: glrt-runnertoken789",
        "CI/CD: glcbt-cicdtoken000",
    ];

    for input in test_cases {
        let output = sanitiser.sanitise(input);
        assert!(
            output.contains("[REDACTED]"),
            "GitLab token should be redacted: {input}"
        );
        assert!(
            !output.contains("glpat-")
                && !output.contains("gloas-")
                && !output.contains("gldt-")
                && !output.contains("glrt-")
                && !output.contains("glcbt-"),
            "Output should not contain GitLab token prefixes"
        );
    }
}

#[test]
fn test_url_credential_redaction() {
    let sanitiser = OutputSanitiser::new();

    let test_cases = [
        (
            "Cloning from https://user:password123@github.com/repo.git",
            "github.com",
            "password123",
        ),
        (
            "Error: https://token:mysecret@gitlab.com/project failed",
            "gitlab.com",
            "mysecret",
        ),
        (
            "Remote: https://deploy:accesskey@bitbucket.org/repo",
            "bitbucket.org",
            "accesskey",
        ),
    ];

    for (input, should_contain, should_not_contain) in test_cases {
        let output = sanitiser.sanitise(input);
        assert!(
            output.contains(should_contain),
            "Output should still contain host: {should_contain}, output was: {output}"
        );
        assert!(
            !output.contains(should_not_contain),
            "Output should not contain credential: {should_not_contain}"
        );
        assert!(
            output.contains("[REDACTED]"),
            "Output should contain redaction marker"
        );
    }
}

#[test]
fn test_authorization_header_redaction() {
    let sanitiser = OutputSanitiser::new();

    let test_cases = [
        "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJ1c2VyIn0.signature",
        "Header: Authorization: token ghp_verysecret",
        "Bearer abc123def456",
    ];

    for input in test_cases {
        let output = sanitiser.sanitise(input);
        assert!(
            output.contains("[REDACTED]"),
            "Authorization header should be redacted: {input}"
        );
    }
}

#[test]
fn test_ssh_key_content_redaction() {
    let sanitiser = OutputSanitiser::new();

    let test_cases = [
        "Key: -----BEGIN RSA PRIVATE KEY-----",
        "Found: -----BEGIN OPENSSH PRIVATE KEY-----",
        "Content: -----END RSA PRIVATE KEY-----",
        "Error: PRIVATE KEY invalid format",
    ];

    for input in test_cases {
        let output = sanitiser.sanitise(input);
        assert!(
            output.contains("[REDACTED]"),
            "SSH key content should be redacted: {input}"
        );
    }
}

#[test]
fn test_safe_output_unchanged() {
    let sanitiser = OutputSanitiser::new();

    let safe_outputs = [
        "Cloning into 'repo'...",
        "remote: Counting objects: 100, done.",
        "Receiving objects: 100% (100/100), done.",
        "Already up to date.",
        "On branch main",
        "nothing to commit, working tree clean",
        "https://github.com/user/repo.git",
        "git@github.com:user/repo.git",
    ];

    for input in safe_outputs {
        let output = sanitiser.sanitise(input);
        assert_eq!(
            output.as_ref(),
            input,
            "Safe output should be unchanged: {input}"
        );
    }
}

#[test]
fn test_multiple_credentials_in_output() {
    let sanitiser = OutputSanitiser::new();

    let input = "Tokens: ghp_first123 and glpat-second456 in https://user:pass@github.com";
    let output = sanitiser.sanitise(input);

    assert!(!output.contains("ghp_first123"));
    assert!(!output.contains("glpat-second456"));
    assert!(!output.contains("user:pass"));
    assert!(output.matches("[REDACTED]").count() >= 3);
}

#[test]
fn test_custom_pattern_redaction() {
    let mut sanitiser = OutputSanitiser::new();
    sanitiser.add_pattern("CUSTOM_SECRET_");

    let input = "Using CUSTOM_SECRET_abc123xyz for authentication";
    let output = sanitiser.sanitise(input);

    assert!(!output.contains("CUSTOM_SECRET_"));
    assert!(output.contains("[REDACTED]"));
}

#[test]
fn test_contains_credentials_detection() {
    let sanitiser = OutputSanitiser::new();

    // Should detect credentials
    assert!(sanitiser.contains_credentials("ghp_token123"));
    assert!(sanitiser.contains_credentials("glpat-secret"));
    assert!(sanitiser.contains_credentials("https://user:pass@host.com/"));
    assert!(sanitiser.contains_credentials("Authorization: Bearer token"));

    // Should not flag safe content
    assert!(!sanitiser.contains_credentials("On branch main"));
    assert!(!sanitiser.contains_credentials("https://github.com/user/repo"));
    assert!(!sanitiser.contains_credentials("Cloning into 'repo'..."));
}

// =============================================================================
// Security Guards Tests
// =============================================================================

#[test]
fn test_branch_guard_protects_main() {
    let guard = BranchGuard::with_defaults();

    // Should block deletion of protected branches
    let result = guard.check("branch", &["-d".to_string(), "main".to_string()]);
    assert!(result.is_blocked());

    let result = guard.check("branch", &["-D".to_string(), "master".to_string()]);
    assert!(result.is_blocked());

    // Should allow deletion of non-protected branches
    let result = guard.check("branch", &["-d".to_string(), "feature".to_string()]);
    assert!(result.is_allowed());
}

#[test]
fn test_branch_guard_blocks_force_push_to_protected() {
    let guard = BranchGuard::with_defaults();

    let result = guard.check(
        "push",
        &[
            "--force".to_string(),
            "origin".to_string(),
            "main".to_string(),
        ],
    );
    assert!(result.is_blocked());

    // Normal push should be allowed
    let result = guard.check("push", &["origin".to_string(), "main".to_string()]);
    assert!(result.is_allowed());
}

#[test]
fn test_push_guard_blocks_force_push() {
    let guard = PushGuard::default(); // Default blocks force push

    let force_flags = ["--force", "-f", "--force-with-lease"];
    for flag in force_flags {
        let result = guard.check("push", &[flag.to_string(), "origin".to_string()]);
        assert!(
            result.is_blocked(),
            "Force push with {flag} should be blocked"
        );
    }

    // Normal push should be allowed
    let result = guard.check("push", &["origin".to_string(), "main".to_string()]);
    assert!(result.is_allowed());
}

#[test]
fn test_push_guard_allows_force_when_configured() {
    let guard = PushGuard::allow_force_push();

    let result = guard.check("push", &["--force".to_string(), "origin".to_string()]);
    assert!(result.is_allowed());
}

#[test]
fn test_repo_filter_blocklist() {
    let mut filter = RepoFilter::blocklist_mode();
    filter.block("github.com/blocked/repo");

    // Blocked repo
    let result = filter.check(
        "clone",
        &["https://github.com/blocked/repo.git".to_string()],
    );
    assert!(result.is_blocked());

    // Allowed repo
    let result = filter.check(
        "clone",
        &["https://github.com/allowed/repo.git".to_string()],
    );
    assert!(result.is_allowed());
}

#[test]
fn test_repo_filter_allowlist() {
    let mut filter = RepoFilter::allowlist_mode();
    filter.allow("github.com/myorg/*");

    // Allowed org
    let result = filter.check("clone", &["https://github.com/myorg/repo1.git".to_string()]);
    assert!(result.is_allowed());

    // Not in allowlist
    let result = filter.check("clone", &["https://github.com/other/repo.git".to_string()]);
    assert!(result.is_blocked());
}

#[test]
fn test_repo_filter_normalises_urls() {
    let mut filter = RepoFilter::blocklist_mode();
    filter.block("github.com/test/repo");

    // All these variations should be blocked
    let blocked_urls = [
        "https://github.com/test/repo.git",
        "http://github.com/test/repo",
        "git@github.com:test/repo.git",
        "HTTPS://GITHUB.COM/TEST/REPO.GIT",
    ];

    for url in blocked_urls {
        let result = filter.check("clone", &[url.to_string()]);
        assert!(result.is_blocked(), "URL {url} should be blocked");
    }
}

// =============================================================================
// Audit Event Security Tests
// =============================================================================

#[test]
fn test_audit_event_does_not_contain_credentials() {
    let event = AuditEvent::command_success(
        "clone",
        vec!["https://github.com/user/repo.git".to_string()],
        None,
        std::time::Duration::from_secs(5),
        0,
    );

    let json = serde_json::to_string(&event).expect("Serialization should succeed");

    // Should not contain any credential patterns
    assert!(!json.contains("ghp_"));
    assert!(!json.contains("glpat-"));
    assert!(!json.contains("password"));
    assert!(!json.contains("secret"));
    assert!(!json.contains("token:"));

    // Should contain expected fields
    assert!(json.contains("\"event_type\":\"command_executed\""));
    assert!(json.contains("\"command\":\"clone\""));
}

#[test]
fn test_audit_blocked_event_safe() {
    let event = AuditEvent::command_blocked(
        "push",
        vec!["--force".to_string(), "origin".to_string()],
        None,
        "Force push is not allowed",
    );

    let json = serde_json::to_string(&event).expect("Serialization should succeed");

    assert!(json.contains("\"outcome\":\"blocked\""));
    assert!(json.contains("Force push is not allowed"));
    assert!(!json.contains("ghp_"));
}

// =============================================================================
// Error Message Security Tests
// =============================================================================

#[test]
fn test_git_command_errors_safe() {
    use git_proxy_mcp::git::command::GitCommandError;

    let errors = [
        GitCommandError::EmptyCommand,
        GitCommandError::CommandNotAllowed {
            command: "config".to_string(),
        },
        GitCommandError::DangerousFlag {
            flag: "--exec".to_string(),
        },
        GitCommandError::InvalidWorkingDirectory {
            path: std::path::PathBuf::from("/invalid"),
        },
    ];

    for error in errors {
        let message = error.to_string();

        // Error messages should not contain credential patterns
        assert!(!message.contains("ghp_"));
        assert!(!message.contains("glpat-"));
        assert!(!message.contains("password"));
    }
}

// =============================================================================
// Rate Limiting Tests
// =============================================================================

#[test]
fn test_rate_limiter_prevents_abuse() {
    use git_proxy_mcp::security::RateLimiter;

    let limiter = RateLimiter::new(3, 0.0); // 3 ops, no refill

    // First 3 should succeed
    assert!(limiter.try_acquire());
    assert!(limiter.try_acquire());
    assert!(limiter.try_acquire());

    // 4th should fail
    assert!(!limiter.try_acquire());

    let stats = limiter.stats();
    assert_eq!(stats.total_allowed, 3);
    assert_eq!(stats.total_blocked, 1);
}

#[test]
fn test_rate_limiter_stats_accurate() {
    use git_proxy_mcp::security::RateLimiter;

    let limiter = RateLimiter::new(2, 0.0);

    limiter.try_acquire(); // allowed
    limiter.try_acquire(); // allowed
    limiter.try_acquire(); // blocked
    limiter.try_acquire(); // blocked

    let stats = limiter.stats();
    assert_eq!(stats.total_allowed, 2);
    assert_eq!(stats.total_blocked, 2);
    assert!((stats.block_rate() - 50.0).abs() < 0.01);
}
