//! Security tests for git-proxy-mcp.
//!
//! These tests verify:
//! - Security guards (branch protection, push guards, repo filters)
//! - Rate limiting
//! - Audit event safety

use git_proxy_mcp::security::audit::AuditEvent;
use git_proxy_mcp::security::guards::{BranchGuard, PushGuard, RepoFilter, SecurityGuard};

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
