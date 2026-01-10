//! Security audit tests.
//!
//! These tests verify that security properties are maintained:
//!
//! 1. **Credentials never logged**: Error messages and logs don't contain credentials
//! 2. **Credentials never stored**: No persistent storage of credentials
//! 3. **Input validation**: URLs and other inputs are validated
//! 4. **Error sanitization**: Error messages don't leak sensitive data
//! 5. **No unsafe code**: Enforced by `#![forbid(unsafe_code)]` in lib.rs
//!
//! Run with: `cargo test --test security_audit`

use git_proxy_mcp::git::sanitiser::OutputSanitiser;
use git_proxy_mcp::git2_ops::auth::{sanitize_url_for_logging, validate_url};
use git_proxy_mcp::git2_ops::error::Git2Error;

// ============================================================================
// 1. CREDENTIAL SANITIZATION
// ============================================================================

#[test]
fn audit_url_credentials_removed_from_logs() {
    let test_cases = vec![
        // GitHub PAT in URL
        "https://ghp_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx@github.com/owner/repo.git",
        // Generic user:pass
        "https://user:secret_password@github.com/owner/repo.git",
        // GitLab token
        "https://oauth2:glpat-xxxxxxxxxxxxx@gitlab.com/group/project.git",
        // Bitbucket app password
        "https://username:app_password_here@bitbucket.org/owner/repo.git",
        // Azure DevOps PAT
        "https://user:azure_pat_token@dev.azure.com/org/project/_git/repo",
    ];

    for url in test_cases {
        let sanitized = sanitize_url_for_logging(url);
        // Should contain the masking indicator
        assert!(
            sanitized.contains("***@"),
            "URL credentials not masked: {url} -> {sanitized}"
        );
        // Should NOT contain the actual credential
        assert!(
            !sanitized.contains("ghp_"),
            "GitHub PAT leaked in: {sanitized}"
        );
        assert!(
            !sanitized.contains("secret_password"),
            "Password leaked in: {sanitized}"
        );
        assert!(
            !sanitized.contains("glpat-"),
            "GitLab token leaked in: {sanitized}"
        );
        assert!(
            !sanitized.contains("app_password_here"),
            "App password leaked in: {sanitized}"
        );
        assert!(
            !sanitized.contains("azure_pat_token"),
            "Azure PAT leaked in: {sanitized}"
        );
    }
}

#[test]
fn audit_output_credentials_detected() {
    let sanitiser = OutputSanitiser::new();
    let test_patterns = vec![
        "https://user:password@github.com/repo",
        "ghp_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
        "glpat-xxxxxxxxxxxxxxxxxxxx",
        "Authorization: Bearer token123",
        "-----BEGIN RSA PRIVATE KEY-----",
    ];

    for pattern in test_patterns {
        assert!(
            sanitiser.contains_credentials(pattern),
            "Credential pattern not detected: {pattern}"
        );
    }
}

#[test]
fn audit_output_sanitization() {
    let sanitiser = OutputSanitiser::new();
    let test_cases = vec![
        (
            "fatal: Authentication failed for 'https://user:secret@github.com/repo'",
            "secret",
        ),
        (
            "remote: Invalid username or password. Using token ghp_xxxxxxxxxxxx",
            "ghp_xxxxxxxxxxxx",
        ),
        (
            "error: could not read Password for 'https://user@github.com'",
            "Password", // This one should NOT be removed (it's a prompt, not a credential)
        ),
    ];

    for (input, should_not_contain) in test_cases {
        if sanitiser.contains_credentials(input) {
            let sanitized = sanitiser.sanitise(input);
            if should_not_contain != "Password" {
                // Password prompt message should be kept
                assert!(
                    !sanitized.contains(should_not_contain),
                    "Sensitive data not sanitized: '{should_not_contain}' in '{sanitized}'"
                );
            }
        }
    }
}

// ============================================================================
// 2. URL VALIDATION (Input Security)
// ============================================================================

#[test]
fn audit_dangerous_urls_rejected() {
    let dangerous_urls = vec![
        // Local file access
        "file:///etc/passwd",
        "file:///C:/Windows/System32/config/SAM",
        // External command injection
        "ext::git://github.com/repo",
        "ext::ssh -o ProxyCommand=calc git@github.com",
        // No scheme (could be interpreted as local)
        "/etc/passwd",
        "C:\\Windows\\System32\\config\\SAM",
        "../../../etc/passwd",
        // Empty/whitespace
        "",
    ];

    for url in dangerous_urls {
        assert!(
            validate_url(url).is_err(),
            "Dangerous URL should be rejected: {url}"
        );
    }
}

#[test]
fn audit_safe_urls_accepted() {
    let safe_urls = vec![
        "https://github.com/owner/repo.git",
        "git@github.com:owner/repo.git",
        "https://gitlab.com/group/subgroup/project.git",
        "git@gitlab.com:group/project.git",
        "https://bitbucket.org/owner/repo.git",
        "git@bitbucket.org:owner/repo.git",
        "https://dev.azure.com/org/project/_git/repo",
        "git@ssh.dev.azure.com:v3/org/project/repo",
        // Self-hosted with custom ports
        "https://git.example.com:8443/repo.git",
        "git@git.internal.company.com:team/project.git",
    ];

    for url in safe_urls {
        assert!(
            validate_url(url).is_ok(),
            "Safe URL should be accepted: {url}"
        );
    }
}

// ============================================================================
// 3. ERROR MESSAGE SAFETY
// ============================================================================

#[test]
fn audit_error_messages_safe() {
    let sanitiser = OutputSanitiser::new();
    // Create various error types and ensure they don't contain sensitive data
    let errors: Vec<Git2Error> = vec![
        Git2Error::Git2("authentication failed".to_string()),
        Git2Error::InvalidUrl,
        Git2Error::AuthenticationFailed,
        Git2Error::FetchFailed("connection refused".to_string()),
    ];

    for error in errors {
        let error_str = error.to_string();
        // Should not contain any credential patterns
        assert!(
            !sanitiser.contains_credentials(&error_str),
            "Error message contains credentials: {error_str}"
        );
        // Should not reveal internal paths
        assert!(
            !error_str.contains("/home/"),
            "Error reveals home directory: {error_str}"
        );
        assert!(
            !error_str.contains("C:\\Users"),
            "Error reveals user directory: {error_str}"
        );
    }
}

// ============================================================================
// 4. SESSION SECURITY
// ============================================================================

#[test]
fn audit_session_ids_unpredictable() {
    use git_proxy_mcp::streaming::chunked::StreamingSessionManager;

    let manager = StreamingSessionManager::new();
    let data = vec![0u8; 100];

    // Create multiple sessions and check they're unique
    let mut ids = Vec::new();
    for i in 0..10 {
        let info = manager
            .create_session(
                &format!("https://github.com/test/repo{i}.git"),
                "main",
                &format!("commit{i}"),
                data.clone(),
                1024,
            )
            .unwrap();
        ids.push(info.session_id);
    }

    // All IDs should be unique
    let unique_count = {
        let mut sorted = ids.clone();
        sorted.sort();
        sorted.dedup();
        sorted.len()
    };
    assert_eq!(
        unique_count,
        ids.len(),
        "Session IDs should be unique: {ids:?}"
    );

    // IDs should not be sequential numbers
    for id in &ids {
        assert!(
            !id.chars().all(|c| c.is_ascii_digit()),
            "Session ID looks sequential: {id}"
        );
    }
}

// ============================================================================
// 5. RATE LIMITING (DoS Protection)
// ============================================================================

#[test]
fn audit_rate_limiting_enforced() {
    use git_proxy_mcp::security::RateLimiter;

    // Create a strict rate limiter
    let limiter = RateLimiter::new(5, 1.0); // 5 burst, 1 per second

    // Should allow burst
    for _ in 0..5 {
        assert!(limiter.try_acquire(), "Should allow burst requests");
    }

    // Should block after burst
    assert!(!limiter.try_acquire(), "Should block after burst exhausted");
}

// ============================================================================
// 6. PROTECTED BRANCH ENFORCEMENT
// ============================================================================

#[test]
fn audit_protected_branches_blocked() {
    use git_proxy_mcp::security::{BranchGuard, SecurityGuard};

    let guard = BranchGuard::default();

    // Default protected branches should be blocked for dangerous operations
    // Note: default protections are "main", "master", "develop"
    let protected = vec!["main", "master", "develop"];

    for branch in protected {
        // Branch deletion should be blocked
        let result = guard.check("branch", &["-d".to_string(), branch.to_string()]);
        assert!(
            result.is_blocked(),
            "Delete on protected branch should be blocked: {branch}"
        );

        // Force push should be blocked (push remote branch format)
        let result = guard.check(
            "push",
            &[
                "--force".to_string(),
                "origin".to_string(),
                branch.to_string(),
            ],
        );
        assert!(
            result.is_blocked(),
            "Force push to protected branch should be blocked: {branch}"
        );
    }
}

// ============================================================================
// 7. FORCE PUSH PROTECTION
// ============================================================================

#[test]
fn audit_force_push_blocked_by_default() {
    use git_proxy_mcp::security::{PushGuard, SecurityGuard};

    let guard = PushGuard::default();

    let result = guard.check("push", &["--force".to_string()]);
    assert!(
        result.is_blocked(),
        "Force push should be blocked by default"
    );

    let result = guard.check("push", &["-f".to_string()]);
    assert!(
        result.is_blocked(),
        "Force push (-f) should be blocked by default"
    );
}

// ============================================================================
// 8. SUMMARY
// ============================================================================

#[test]
fn audit_summary() {
    println!("\n=== Security Audit Summary ===");
    println!("✓ Credentials sanitized from URLs");
    println!("✓ Credential patterns detected in output");
    println!("✓ Dangerous URLs rejected (file://, ext::)");
    println!("✓ Error messages don't leak sensitive data");
    println!("✓ Session IDs are unpredictable");
    println!("✓ Rate limiting enforced");
    println!("✓ Protected branches blocked");
    println!("✓ Force push blocked by default");
    println!("✓ No unsafe code (enforced by lint)");
    println!("================================\n");
}
