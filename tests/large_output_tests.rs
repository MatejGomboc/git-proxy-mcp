//! Tests for handling large git command output.
//!
//! These tests verify that the system correctly handles large output from git
//! commands, ensuring:
//!
//! 1. Large stdout/stderr is captured and processed correctly
//! 2. Sanitisation works efficiently on large outputs
//! 3. Memory is managed properly with large data
//! 4. No panics or truncation issues with boundary cases

use std::borrow::Cow;
use std::fmt::Write;

use git_proxy_mcp::git::sanitiser::OutputSanitiser;

// =============================================================================
// Constants for Test Sizes
// =============================================================================

/// Size for small output tests (typical git command output).
const SMALL_OUTPUT_SIZE: usize = 1024; // 1 KB

/// Size for medium output tests (large diffs, many refs).
const MEDIUM_OUTPUT_SIZE: usize = 1024 * 100; // 100 KB

/// Size for large output tests (very large repos, many branches).
const LARGE_OUTPUT_SIZE: usize = 1024 * 1024; // 1 MB

/// Size for very large output tests (stress testing).
const VERY_LARGE_OUTPUT_SIZE: usize = 1024 * 1024 * 10; // 10 MB

// =============================================================================
// Helper Functions
// =============================================================================

/// Generates a string of the specified size with realistic git-like content.
fn generate_git_output(size: usize) -> String {
    let line = "abc123def456 refs/heads/feature-branch-name\n";
    let line_len = line.len();
    let repetitions = size / line_len;

    let mut output = String::with_capacity(size);
    for i in 0..repetitions {
        // Vary the content slightly to be more realistic
        let _ = writeln!(output, "{i:040x} refs/heads/feature-branch-{}", i % 1000);
    }
    output
}

/// Generates output with embedded credentials at various positions.
fn generate_output_with_credentials(size: usize, credential_count: usize) -> String {
    let safe_line = "abc123def456 refs/heads/feature-branch-name\n";
    let credential_line = "error: https://user:ghp_secret12345678901234567@github.com failed\n";

    let mut output = String::with_capacity(size);
    let lines_needed = size / safe_line.len();
    let credential_interval = lines_needed / (credential_count + 1);

    for i in 0..lines_needed {
        if credential_count > 0 && i > 0 && i % credential_interval == 0 {
            output.push_str(credential_line);
        } else {
            let _ = writeln!(output, "{i:040x} refs/heads/feature-branch-{}", i % 1000);
        }
    }
    output
}

/// Generates output with many different credential types.
fn generate_output_with_mixed_credentials(size: usize) -> String {
    let credentials = [
        "token: ghp_1234567890abcdefghijklmnopqrstuv\n",
        "gitlab: glpat-abcdefghijk123456789\n",
        "url: https://user:password123@github.com/repo\n",
        "header: Authorization: Bearer eyJhbGciOiJ...\n",
        "ssh: -----BEGIN RSA PRIVATE KEY-----\n",
        "azure: azure://user:secret@dev.azure.com\n",
    ];

    let safe_line = "abc123def456 refs/heads/feature-branch-name\n";
    let mut output = String::with_capacity(size);
    let mut cred_index = 0;

    while output.len() < size {
        // Add some safe lines
        for _ in 0..10 {
            if output.len() >= size {
                break;
            }
            output.push_str(safe_line);
        }
        // Add a credential line
        if output.len() < size {
            output.push_str(credentials[cred_index % credentials.len()]);
            cred_index += 1;
        }
    }
    output.truncate(size);
    output
}

// =============================================================================
// Basic Large Output Tests
// =============================================================================

#[test]
fn test_sanitiser_handles_small_output() {
    let sanitiser = OutputSanitiser::new();
    let input = generate_git_output(SMALL_OUTPUT_SIZE);

    let start = std::time::Instant::now();
    let output = sanitiser.sanitise(&input);
    let elapsed = start.elapsed();

    // Should complete quickly (under 10ms for 1KB)
    assert!(
        elapsed.as_millis() < 10,
        "Sanitisation took too long: {elapsed:?}"
    );

    // No credentials in safe output, should be borrowed
    assert!(
        matches!(output, Cow::Borrowed(_)),
        "Safe output should be borrowed, not cloned"
    );
}

#[test]
fn test_sanitiser_handles_medium_output() {
    let sanitiser = OutputSanitiser::new();
    let input = generate_git_output(MEDIUM_OUTPUT_SIZE);

    let start = std::time::Instant::now();
    let output = sanitiser.sanitise(&input);
    let elapsed = start.elapsed();

    // Should complete in reasonable time (under 100ms for 100KB)
    assert!(
        elapsed.as_millis() < 100,
        "Sanitisation took too long: {elapsed:?}"
    );

    // Should be borrowed (no changes needed)
    assert!(matches!(output, Cow::Borrowed(_)));
    assert_eq!(output.len(), input.len());
}

#[test]
fn test_sanitiser_handles_large_output() {
    let sanitiser = OutputSanitiser::new();
    let input = generate_git_output(LARGE_OUTPUT_SIZE);

    let start = std::time::Instant::now();
    let output = sanitiser.sanitise(&input);
    let elapsed = start.elapsed();

    // Should complete in reasonable time (under 5s for 1MB)
    // CI environments can be significantly slower than local machines
    assert!(
        elapsed.as_secs() < 5,
        "Sanitisation took too long: {elapsed:?}"
    );

    // Should be borrowed (no changes needed)
    assert!(matches!(output, Cow::Borrowed(_)));
}

#[test]
fn test_sanitiser_handles_very_large_output() {
    let sanitiser = OutputSanitiser::new();
    let input = generate_git_output(VERY_LARGE_OUTPUT_SIZE);

    let start = std::time::Instant::now();
    let output = sanitiser.sanitise(&input);
    let elapsed = start.elapsed();

    // Should complete in reasonable time (under 30s for 10MB)
    // CI environments can be significantly slower than local machines
    assert!(
        elapsed.as_secs() < 30,
        "Sanitisation took too long: {elapsed:?}"
    );

    // Should be borrowed (no changes needed)
    assert!(matches!(output, Cow::Borrowed(_)));
}

// =============================================================================
// Large Output with Credentials Tests
// =============================================================================

#[test]
fn test_sanitiser_handles_large_output_with_single_credential() {
    let sanitiser = OutputSanitiser::new();
    let input = generate_output_with_credentials(MEDIUM_OUTPUT_SIZE, 1);

    let start = std::time::Instant::now();
    let output = sanitiser.sanitise(&input);
    let elapsed = start.elapsed();

    // Should complete in reasonable time
    assert!(
        elapsed.as_millis() < 200,
        "Sanitisation took too long: {elapsed:?}"
    );

    // Should be owned (changes were made)
    assert!(matches!(output, Cow::Owned(_)));

    // Credential should be redacted
    assert!(!output.contains("ghp_"), "GitHub PAT should be redacted");
    assert!(
        output.contains("[REDACTED]"),
        "Should contain redaction marker"
    );
}

#[test]
fn test_sanitiser_handles_large_output_with_many_credentials() {
    let sanitiser = OutputSanitiser::new();
    let input = generate_output_with_credentials(LARGE_OUTPUT_SIZE, 100);

    let start = std::time::Instant::now();
    let output = sanitiser.sanitise(&input);
    let elapsed = start.elapsed();

    // Should complete in reasonable time (even with 100 credentials)
    assert!(
        elapsed.as_secs() < 2,
        "Sanitisation took too long: {elapsed:?}"
    );

    // All credentials should be redacted
    assert!(
        !output.contains("ghp_"),
        "All GitHub PATs should be redacted"
    );

    // Should have many redaction markers
    let redaction_count = output.matches("[REDACTED]").count();
    assert!(
        redaction_count >= 100,
        "Expected at least 100 redactions, got {redaction_count}"
    );
}

#[test]
fn test_sanitiser_handles_large_output_with_mixed_credentials() {
    let sanitiser = OutputSanitiser::new();
    let input = generate_output_with_mixed_credentials(MEDIUM_OUTPUT_SIZE);

    let output = sanitiser.sanitise(&input);

    // All credential types should be redacted
    assert!(!output.contains("ghp_"), "GitHub PAT should be redacted");
    assert!(!output.contains("glpat-"), "GitLab PAT should be redacted");
    assert!(
        !output.contains("password123"),
        "URL password should be redacted"
    );
    assert!(!output.contains("-----BEGIN"), "SSH key should be redacted");
}

// =============================================================================
// Edge Cases and Boundary Tests
// =============================================================================

#[test]
fn test_sanitiser_handles_empty_output() {
    let sanitiser = OutputSanitiser::new();
    let input = "";

    let output = sanitiser.sanitise(input);

    assert!(matches!(output, Cow::Borrowed(_)));
    assert!(output.is_empty());
}

#[test]
fn test_sanitiser_handles_single_byte_output() {
    let sanitiser = OutputSanitiser::new();
    let input = "x";

    let output = sanitiser.sanitise(input);

    assert!(matches!(output, Cow::Borrowed(_)));
    assert_eq!(output.as_ref(), "x");
}

#[test]
fn test_sanitiser_handles_output_with_only_newlines() {
    let sanitiser = OutputSanitiser::new();
    let input = "\n".repeat(MEDIUM_OUTPUT_SIZE);

    let output = sanitiser.sanitise(&input);

    assert!(matches!(output, Cow::Borrowed(_)));
    assert_eq!(output.len(), input.len());
}

#[test]
fn test_sanitiser_handles_output_with_unicode() {
    let sanitiser = OutputSanitiser::new();
    // Mix of ASCII and Unicode characters
    let mut input = String::new();
    for i in 0..1000 {
        let _ = writeln!(input, "行{i}: 🔒 安全なメッセージ 日本語テスト");
    }
    input.push_str("token: ghp_secret12345678901234567890\n");

    let output = sanitiser.sanitise(&input);

    // Credential should be redacted
    assert!(!output.contains("ghp_"));
    // Unicode should be preserved
    assert!(output.contains("安全"));
    assert!(output.contains("🔒"));
}

#[test]
fn test_sanitiser_handles_very_long_lines() {
    let sanitiser = OutputSanitiser::new();

    // Create a single very long line
    let long_line = "a".repeat(MEDIUM_OUTPUT_SIZE);
    let input = format!("{long_line}\nghp_secret12345678901234567890\n");

    let output = sanitiser.sanitise(&input);

    // Long line should be preserved
    assert!(output.contains(&"a".repeat(1000)));
    // Credential should be redacted
    assert!(!output.contains("ghp_"));
}

#[test]
fn test_sanitiser_handles_credential_at_start() {
    let sanitiser = OutputSanitiser::new();
    let input = format!(
        "ghp_secret12345\n{}",
        generate_git_output(SMALL_OUTPUT_SIZE)
    );

    let output = sanitiser.sanitise(&input);

    assert!(!output.contains("ghp_"));
    assert!(output.starts_with("[REDACTED]"));
}

#[test]
fn test_sanitiser_handles_credential_at_end() {
    let sanitiser = OutputSanitiser::new();
    let input = format!(
        "{}\nghp_secret12345",
        generate_git_output(SMALL_OUTPUT_SIZE)
    );

    let output = sanitiser.sanitise(&input);

    assert!(!output.contains("ghp_"));
    assert!(output.ends_with("[REDACTED]"));
}

#[test]
fn test_sanitiser_handles_adjacent_credentials() {
    let sanitiser = OutputSanitiser::new();
    let input = "ghp_first123 ghp_second456 glpat-third789";

    let output = sanitiser.sanitise(input);

    assert!(!output.contains("ghp_"));
    assert!(!output.contains("glpat-"));
    assert_eq!(output.matches("[REDACTED]").count(), 3);
}

#[test]
fn test_sanitiser_handles_nested_url_credentials() {
    let sanitiser = OutputSanitiser::new();
    // URL within a larger message
    let input =
        "Failed to fetch https://user:ghp_secret123@github.com/repo.git due to network error";

    let output = sanitiser.sanitise(input);

    assert!(!output.contains("ghp_secret123"));
    // Either the PAT or the URL credentials should be redacted
    assert!(output.contains("[REDACTED]"));
}

// =============================================================================
// Memory Safety Tests
// =============================================================================

#[test]
fn test_sanitiser_does_not_allocate_for_safe_output() {
    let sanitiser = OutputSanitiser::new();
    let input = generate_git_output(MEDIUM_OUTPUT_SIZE);

    let output = sanitiser.sanitise(&input);

    // Verify it's borrowed (no heap allocation for the result)
    assert!(
        matches!(output, Cow::Borrowed(_)),
        "Safe output should not trigger allocation"
    );

    // The borrowed reference should point to the same memory
    if let Cow::Borrowed(s) = output {
        assert!(std::ptr::eq(s.as_ptr(), input.as_ptr()));
    }
}

#[test]
fn test_sanitiser_handles_repeated_sanitisation() {
    let sanitiser = OutputSanitiser::new();
    let input = generate_output_with_credentials(MEDIUM_OUTPUT_SIZE, 10);

    // Sanitise multiple times (should be idempotent after first pass)
    let output1 = sanitiser.sanitise(&input);
    let output2 = sanitiser.sanitise(&output1);
    let output3 = sanitiser.sanitise(&output2);

    // All should have the same content
    assert_eq!(output2.as_ref(), output1.as_ref());
    assert_eq!(output3.as_ref(), output2.as_ref());

    // After first sanitisation, subsequent ones should be borrowed
    assert!(matches!(output2, Cow::Borrowed(_)));
    assert!(matches!(output3, Cow::Borrowed(_)));
}

// =============================================================================
// Performance Benchmarks (as tests)
// =============================================================================

#[test]
#[allow(clippy::cast_precision_loss)]
fn test_sanitiser_performance_scaling() {
    let sanitiser = OutputSanitiser::new();

    // Test that performance scales roughly linearly
    let sizes = [
        SMALL_OUTPUT_SIZE,
        SMALL_OUTPUT_SIZE * 10,
        SMALL_OUTPUT_SIZE * 100,
    ];
    let mut times = Vec::new();

    for &size in &sizes {
        let input = generate_git_output(size);
        let start = std::time::Instant::now();
        let _ = sanitiser.sanitise(&input);
        times.push(start.elapsed().as_micros() as f64);
    }

    // The time should roughly scale with size (allowing for 5x variance)
    // time[1] / time[0] should be roughly 10 (within 5x factor)
    // time[2] / time[1] should be roughly 10 (within 5x factor)
    if times[0] > 0.0 {
        let ratio1 = times[1] / times[0];
        let ratio2 = times[2] / times[1];

        // Allow for significant variance due to system load, but should
        // roughly follow linear scaling (not quadratic)
        assert!(
            ratio1 < 50.0,
            "Scaling from small to medium is worse than expected: {ratio1}x"
        );
        assert!(
            ratio2 < 50.0,
            "Scaling from medium to large is worse than expected: {ratio2}x"
        );
    }
}

#[test]
fn test_contains_credentials_performance() {
    let sanitiser = OutputSanitiser::new();
    let input = generate_git_output(LARGE_OUTPUT_SIZE);

    let start = std::time::Instant::now();
    for _ in 0..10 {
        let _ = sanitiser.contains_credentials(&input);
    }
    let elapsed = start.elapsed();

    // 10 checks on 1MB should complete in under 30 seconds
    // (conservative limit to account for slow CI environments)
    assert!(
        elapsed.as_secs() < 30,
        "contains_credentials is too slow: {elapsed:?}"
    );
}

// =============================================================================
// Integration with CommandOutput Structure
// =============================================================================

#[test]
fn test_large_stdout_and_stderr_together() {
    let sanitiser = OutputSanitiser::new();

    // Simulate large stdout and stderr
    let stdout = generate_git_output(MEDIUM_OUTPUT_SIZE);
    let stderr = format!(
        "warning: many refs\n{}\nerror: ghp_secret123",
        generate_git_output(SMALL_OUTPUT_SIZE)
    );

    let sanitised_stdout = sanitiser.sanitise(&stdout);
    let sanitised_stderr = sanitiser.sanitise(&stderr);

    // stdout should be borrowed (no credentials)
    assert!(matches!(sanitised_stdout, Cow::Borrowed(_)));

    // stderr should be owned (credential redacted)
    assert!(matches!(sanitised_stderr, Cow::Owned(_)));
    assert!(!sanitised_stderr.contains("ghp_"));
}

// =============================================================================
// Stress Tests
// =============================================================================

#[test]
fn test_many_small_sanitisations() {
    let sanitiser = OutputSanitiser::new();

    // Simulate many small outputs (like multiple git operations)
    for i in 0..1000 {
        let input = format!("{i:040x} refs/heads/branch-{i}\n");
        let output = sanitiser.sanitise(&input);
        assert!(matches!(output, Cow::Borrowed(_)));
    }
}

#[test]
fn test_alternating_safe_and_unsafe_outputs() {
    let sanitiser = OutputSanitiser::new();

    for i in 0..100 {
        let input = if i % 2 == 0 {
            format!("safe output line {i}\n")
        } else {
            format!("ghp_secret{i:010} leaked\n")
        };

        let output = sanitiser.sanitise(&input);

        if i % 2 == 0 {
            assert!(matches!(output, Cow::Borrowed(_)));
        } else {
            assert!(!output.contains("ghp_"));
        }
    }
}
