//! Integration tests for the full MCP → Git pipeline.
//!
//! These tests verify the complete request/response cycle from MCP protocol
//! through to git command execution and back. Unlike the individual module
//! tests, these exercise all layers working together:
//!
//! 1. MCP protocol parsing
//! 2. Server request handling
//! 3. Security guard checks
//! 4. Git command validation
//! 5. Git subprocess execution
//! 6. Output sanitisation
//! 7. Response formatting
//!
//! These tests require git to be available on the system.

use std::path::PathBuf;
use std::process::Command;

use serde_json::{json, Value};
use tempfile::TempDir;

use git_proxy_mcp::git::executor::GitExecutor;
use git_proxy_mcp::mcp::protocol::{parse_message, IncomingMessage, RequestId};
use git_proxy_mcp::mcp::server::{McpServer, SecurityConfig, ToolCallResult};
use git_proxy_mcp::security::AuditLogger;

// =============================================================================
// Test Helpers
// =============================================================================

/// Helper to check if git is available on the system.
fn git_available() -> bool {
    Command::new("git").arg("--version").output().is_ok()
}

/// Creates a test server with default configuration.
fn create_test_server() -> McpServer {
    let executor = GitExecutor::new();
    let security_config = SecurityConfig::default();
    let audit_logger = AuditLogger::disabled();

    McpServer::new(executor, security_config, audit_logger)
}

/// Creates a test server with custom security configuration.
fn create_server_with_security(config: SecurityConfig) -> McpServer {
    let executor = GitExecutor::new();
    let audit_logger = AuditLogger::disabled();

    McpServer::new(executor, config, audit_logger)
}

/// Helper to create a temporary git repository for testing.
fn create_temp_repo() -> Option<TempDir> {
    let temp_dir = TempDir::new().ok()?;

    // Initialise a git repository
    let status = Command::new("git")
        .args(["init"])
        .current_dir(temp_dir.path())
        .output()
        .ok()?;

    if !status.status.success() {
        return None;
    }

    // Configure git user for commits
    Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(temp_dir.path())
        .output()
        .ok()?;

    Command::new("git")
        .args(["config", "user.name", "Test User"])
        .current_dir(temp_dir.path())
        .output()
        .ok()?;

    Some(temp_dir)
}

/// Helper to create a bare repository (for testing fetch/pull).
fn create_bare_repo() -> Option<TempDir> {
    let temp_dir = TempDir::new().ok()?;

    let status = Command::new("git")
        .args(["init", "--bare"])
        .current_dir(temp_dir.path())
        .output()
        .ok()?;

    if !status.status.success() {
        return None;
    }

    Some(temp_dir)
}

/// Simulates the full MCP lifecycle: initialize → initialized → running.
///
/// This is a simplified simulation since we can't easily drive the server's
/// async run loop in tests. Instead, we test the individual components.
fn simulate_mcp_lifecycle_messages() -> (String, String, String) {
    let initialize_request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "test-client",
                "version": "1.0.0"
            }
        }
    })
    .to_string();

    let initialized_notification = json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    })
    .to_string();

    let tools_list_request = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/list"
    })
    .to_string();

    (
        initialize_request,
        initialized_notification,
        tools_list_request,
    )
}

/// Creates a tools/call request for a git command.
fn create_git_tool_call(id: i64, command: &str, args: &[&str], cwd: Option<&str>) -> String {
    let mut arguments = json!({
        "command": command,
        "args": args
    });

    if let Some(dir) = cwd {
        arguments["cwd"] = json!(dir);
    }

    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "tools/call",
        "params": {
            "name": "git",
            "arguments": arguments
        }
    })
    .to_string()
}

// =============================================================================
// MCP Protocol Integration Tests
// =============================================================================

#[test]
fn test_mcp_lifecycle_messages_parse_correctly() {
    let (init_req, init_notif, tools_list) = simulate_mcp_lifecycle_messages();

    // Parse initialize request
    let result = parse_message(&init_req);
    assert!(result.is_ok(), "Initialize request should parse");
    if let Ok(IncomingMessage::Request(req)) = result {
        assert_eq!(req.method, "initialize");
        assert_eq!(req.id, RequestId::Number(1));
    } else {
        panic!("Expected initialize request");
    }

    // Parse initialized notification
    let result = parse_message(&init_notif);
    assert!(result.is_ok(), "Initialized notification should parse");
    if let Ok(IncomingMessage::Notification(notif)) = result {
        assert_eq!(notif.method, "notifications/initialized");
    } else {
        panic!("Expected initialized notification");
    }

    // Parse tools/list request
    let result = parse_message(&tools_list);
    assert!(result.is_ok(), "Tools list request should parse");
    if let Ok(IncomingMessage::Request(req)) = result {
        assert_eq!(req.method, "tools/list");
    } else {
        panic!("Expected tools/list request");
    }
}

#[test]
fn test_git_tool_call_message_parses_correctly() {
    let message = create_git_tool_call(3, "fetch", &["origin"], Some("/tmp/repo"));

    let result = parse_message(&message);
    assert!(result.is_ok(), "Tool call should parse");

    if let Ok(IncomingMessage::Request(req)) = result {
        assert_eq!(req.method, "tools/call");
        assert_eq!(req.id, RequestId::Number(3));

        let params = req.params.expect("Should have params");
        assert_eq!(params["name"], "git");
        assert_eq!(params["arguments"]["command"], "fetch");
        assert_eq!(params["arguments"]["args"][0], "origin");
        assert_eq!(params["arguments"]["cwd"], "/tmp/repo");
    } else {
        panic!("Expected tools/call request");
    }
}

#[test]
fn test_git_tool_call_without_cwd_parses_correctly() {
    let message = create_git_tool_call(4, "ls-remote", &["https://github.com/user/repo"], None);

    let result = parse_message(&message);
    assert!(result.is_ok());

    if let Ok(IncomingMessage::Request(req)) = result {
        let params = req.params.expect("Should have params");
        assert!(params["arguments"]["cwd"].is_null());
    }
}

// =============================================================================
// Server State Tests
// =============================================================================

#[test]
fn test_server_starts_in_awaiting_init_state() {
    use git_proxy_mcp::mcp::server::ServerState;

    let server = create_test_server();
    assert_eq!(server.state(), ServerState::AwaitingInit);
}

#[test]
fn test_server_with_custom_security_config() {
    let config = SecurityConfig {
        allow_force_push: true,
        protected_branches: vec!["main".to_string(), "release/*".to_string()],
        repo_allowlist: Some(vec!["github.com/myorg/*".to_string()]),
        repo_blocklist: None,
    };

    let server = create_server_with_security(config);
    // Server should be created successfully with custom config
    assert_eq!(
        server.state(),
        git_proxy_mcp::mcp::server::ServerState::AwaitingInit
    );
}

// =============================================================================
// Git Executor Integration Tests (via Server)
// =============================================================================

#[tokio::test]
async fn test_executor_runs_ls_remote_on_public_repo() {
    if !git_available() {
        eprintln!("Skipping test: git not available");
        return;
    }

    let executor = GitExecutor::new();

    // Use a well-known public repository that should always be available
    let command = git_proxy_mcp::git::command::GitCommand::new(
        "ls-remote",
        vec![
            "--heads".to_string(),
            "https://github.com/git/git.git".to_string(),
        ],
        None,
    )
    .expect("ls-remote command should be valid");

    let result = executor.execute(&command).await;

    match result {
        Ok(output) => {
            // Should succeed (exit code 0)
            assert!(output.success, "ls-remote should succeed");
            // Should have some output (refs)
            assert!(
                !output.stdout.is_empty() || !output.stderr.is_empty(),
                "Should have output"
            );
            // Output should be sanitised (no credentials should appear)
            assert!(
                !output.stdout.contains("ghp_"),
                "Output should not contain GitHub tokens"
            );
        }
        Err(e) => {
            // Network errors are acceptable in CI environments
            eprintln!("ls-remote failed (may be network issue): {e}");
        }
    }
}

#[tokio::test]
async fn test_executor_fetch_in_local_repo() {
    if !git_available() {
        eprintln!("Skipping test: git not available");
        return;
    }

    // Create a repo with a remote
    let Some(repo_dir) = create_temp_repo() else {
        eprintln!("Skipping test: failed to create temp repo");
        return;
    };

    let Some(bare_dir) = create_bare_repo() else {
        eprintln!("Skipping test: failed to create bare repo");
        return;
    };

    // Add the bare repo as a remote
    let add_remote = Command::new("git")
        .args(["remote", "add", "origin", bare_dir.path().to_str().unwrap()])
        .current_dir(repo_dir.path())
        .output();

    assert!(
        add_remote.is_ok() && add_remote.unwrap().status.success(),
        "Should add remote"
    );

    let executor = GitExecutor::new();

    let command = git_proxy_mcp::git::command::GitCommand::new(
        "fetch",
        vec!["origin".to_string()],
        Some(repo_dir.path().to_path_buf()),
    )
    .expect("fetch command should be valid");

    let result = executor.execute(&command).await;

    match result {
        Ok(output) => {
            // Fetch from empty bare repo should succeed
            assert!(output.success, "fetch should succeed: {output:?}");
        }
        Err(e) => {
            panic!("fetch should not error: {e}");
        }
    }
}

#[tokio::test]
async fn test_executor_rejects_nonexistent_working_directory() {
    let executor = GitExecutor::new();

    // Use a platform-appropriate absolute path that doesn't exist
    #[cfg(windows)]
    let nonexistent_path = PathBuf::from("C:\\this\\path\\should\\not\\exist\\anywhere\\test");
    #[cfg(not(windows))]
    let nonexistent_path = PathBuf::from("/this/path/should/not/exist/anywhere/test");

    let command = git_proxy_mcp::git::command::GitCommand::new(
        "fetch",
        vec!["origin".to_string()],
        Some(nonexistent_path),
    )
    .expect("command should be valid (path validation happens in executor)");

    let result = executor.execute(&command).await;

    assert!(result.is_err(), "Should fail for nonexistent directory");
    let err = result.unwrap_err();
    assert!(
        err.to_string().contains("directory"),
        "Error should mention directory: {err}"
    );
}

// =============================================================================
// Security Guard Integration Tests
// =============================================================================

#[test]
fn test_security_config_blocks_force_push_by_default() {
    let config = SecurityConfig::default();
    assert!(
        !config.allow_force_push,
        "Force push should be blocked by default"
    );
}

#[test]
fn test_branch_guard_integration_with_wildcards() {
    use git_proxy_mcp::security::guards::{BranchGuard, SecurityGuard};

    // Test wildcard patterns like release/*
    let guard = BranchGuard::new(vec![
        "main".to_string(),
        "master".to_string(),
        "release/*".to_string(),
    ]);

    // Should block push to protected branches
    let result = guard.check(
        "push",
        &[
            "--force".to_string(),
            "origin".to_string(),
            "main".to_string(),
        ],
    );
    assert!(result.is_blocked(), "Force push to main should be blocked");

    // Should block push to release branches
    let result = guard.check(
        "push",
        &[
            "--force".to_string(),
            "origin".to_string(),
            "release/v1.0".to_string(),
        ],
    );
    assert!(
        result.is_blocked(),
        "Force push to release/* should be blocked"
    );

    // Should allow push to feature branches
    let result = guard.check(
        "push",
        &[
            "--force".to_string(),
            "origin".to_string(),
            "feature/my-feature".to_string(),
        ],
    );
    assert!(
        result.is_allowed(),
        "Force push to feature branch should be allowed (branch guard only protects specific branches)"
    );
}

#[test]
fn test_repo_filter_integration() {
    use git_proxy_mcp::security::guards::{RepoFilter, SecurityGuard};

    // Test allowlist mode
    let mut filter = RepoFilter::allowlist_mode();
    filter.allow("github.com/myorg/*");
    filter.allow("github.com/trusted-repo");

    // Allowed repos
    let result = filter.check(
        "clone",
        &["https://github.com/myorg/any-repo.git".to_string()],
    );
    assert!(result.is_allowed(), "Org repos should be allowed");

    let result = filter.check(
        "clone",
        &["https://github.com/trusted-repo.git".to_string()],
    );
    assert!(result.is_allowed(), "Trusted repo should be allowed");

    // Blocked repos
    let result = filter.check("clone", &["https://github.com/random/repo.git".to_string()]);
    assert!(result.is_blocked(), "Random repos should be blocked");
}

#[test]
fn test_rate_limiter_integration() {
    use git_proxy_mcp::security::RateLimiter;

    // Test with burst limit
    let limiter = RateLimiter::new(5, 0.0); // 5 ops, no refill for testing

    // First 5 should succeed
    for i in 0..5 {
        assert!(limiter.try_acquire(), "Request {} should be allowed", i + 1);
    }

    // 6th should fail
    assert!(!limiter.try_acquire(), "Request 6 should be blocked");

    // Stats should be accurate
    let stats = limiter.stats();
    assert_eq!(stats.total_allowed, 5);
    assert_eq!(stats.total_blocked, 1);
}

// =============================================================================
// Tool Call Result Tests
// =============================================================================

#[test]
fn test_tool_call_result_text_serialisation() {
    let result = ToolCallResult::text("Command output here");

    let json = serde_json::to_value(&result).expect("Should serialise");

    // isError is skipped when false (per MCP spec), so it should be absent
    assert!(
        json.get("isError").is_none(),
        "isError should not be present when false"
    );
    assert_eq!(json["content"][0]["type"], "text");
    assert_eq!(json["content"][0]["text"], "Command output here");
}

#[test]
fn test_tool_call_result_error_serialisation() {
    let result = ToolCallResult::error("Something went wrong");

    let json = serde_json::to_value(&result).expect("Should serialise");

    assert!(json["isError"].as_bool().unwrap_or(false));
    assert_eq!(json["content"][0]["text"], "Something went wrong");
}

#[test]
fn test_tool_call_result_no_credentials_in_output() {
    // Simulate output that might contain credentials
    let dangerous_outputs = [
        "ghp_1234567890abcdefghijklmnop",
        "glpat-secrettoken123",
        "https://user:password@github.com",
        "Authorization: Bearer secret",
    ];

    for output in dangerous_outputs {
        let result = ToolCallResult::text(output);
        let json = serde_json::to_value(&result).expect("Should serialise");
        let text = json["content"][0]["text"].as_str().unwrap();

        // Note: ToolCallResult itself doesn't sanitise - that's done in the executor
        // This test documents the current behaviour
        assert_eq!(text, output, "ToolCallResult passes through text as-is");
    }
}

// =============================================================================
// Output Sanitisation Integration Tests
// =============================================================================

#[test]
fn test_sanitiser_handles_real_git_output_patterns() {
    use git_proxy_mcp::git::sanitiser::OutputSanitiser;

    let sanitiser = OutputSanitiser::new();

    // Patterns that might appear in real git output
    let test_cases = vec![
        // Clone with embedded credentials (should be sanitised)
        (
            "Cloning into 'repo'...\nremote: https://user:ghp_secret123@github.com/user/repo.git",
            true, // should be sanitised
        ),
        // Normal clone output (should not be changed)
        (
            "Cloning into 'repo'...\nremote: Counting objects: 100, done.",
            false,
        ),
        // Fetch with token in error (should be sanitised)
        (
            "fatal: could not read Username for 'https://github.com': terminal prompts disabled\nToken: ghp_abc123",
            true,
        ),
        // Normal fetch output (should not be changed)
        (
            "From github.com:user/repo\n * branch main -> FETCH_HEAD",
            false,
        ),
    ];

    for (input, should_be_sanitised) in test_cases {
        let output = sanitiser.sanitise(input);

        if should_be_sanitised {
            assert!(output.contains("[REDACTED]"), "Should sanitise: {input}");
        } else {
            assert_eq!(
                output.as_ref(),
                input,
                "Should not change safe output: {input}"
            );
        }
    }
}

// =============================================================================
// Command Validation Integration Tests
// =============================================================================

#[test]
fn test_git_command_validation_for_mcp_tool() {
    use git_proxy_mcp::git::command::GitCommand;

    // Valid MCP tool commands (remote operations only)
    let valid_commands = vec![
        ("clone", vec!["https://github.com/user/repo.git"]),
        ("fetch", vec!["origin"]),
        ("pull", vec!["origin", "main"]),
        ("push", vec!["origin", "main"]),
        ("ls-remote", vec!["https://github.com/user/repo.git"]),
    ];

    for (cmd, args) in valid_commands {
        let args: Vec<String> = args.into_iter().map(String::from).collect();
        let result = GitCommand::new(cmd, args, None);
        assert!(
            result.is_ok(),
            "Command '{cmd}' should be valid for MCP tool"
        );
    }

    // Invalid commands (local operations - should run directly, not through proxy)
    let invalid_commands = vec![
        "status", "log", "diff", "commit", "add", "branch", "checkout", "merge", "rebase",
    ];

    for cmd in invalid_commands {
        let result = GitCommand::new(cmd, vec![], None);
        assert!(
            result.is_err(),
            "Local command '{cmd}' should be rejected by MCP proxy"
        );
    }
}

#[test]
fn test_dangerous_flags_blocked_in_pipeline() {
    use git_proxy_mcp::git::command::GitCommand;

    let dangerous_flags = vec![
        (
            "clone",
            vec!["--exec=malicious", "https://github.com/user/repo.git"],
        ),
        ("fetch", vec!["-c", "core.sshCommand=evil"]),
        ("push", vec!["--upload-pack=bad", "origin"]),
        ("pull", vec!["--receive-pack=bad", "origin"]),
    ];

    for (cmd, args) in dangerous_flags {
        let args: Vec<String> = args.into_iter().map(String::from).collect();
        let result = GitCommand::new(cmd, args, None);
        assert!(
            result.is_err(),
            "Command '{cmd}' with dangerous flags should be rejected"
        );
    }
}

// =============================================================================
// End-to-End Pipeline Simulation Tests
// =============================================================================

#[tokio::test]
async fn test_full_pipeline_fetch_in_local_repo() {
    if !git_available() {
        eprintln!("Skipping test: git not available");
        return;
    }

    // Set up: create a repo with a remote
    let Some(repo_dir) = create_temp_repo() else {
        eprintln!("Skipping test: failed to create temp repo");
        return;
    };

    let Some(bare_dir) = create_bare_repo() else {
        eprintln!("Skipping test: failed to create bare repo");
        return;
    };

    // Add the bare repo as a remote
    Command::new("git")
        .args(["remote", "add", "origin", bare_dir.path().to_str().unwrap()])
        .current_dir(repo_dir.path())
        .output()
        .expect("Should add remote");

    // Simulate the pipeline:
    // 1. Parse the MCP message
    let message = create_git_tool_call(
        5,
        "fetch",
        &["origin"],
        Some(repo_dir.path().to_str().unwrap()),
    );

    let parsed = parse_message(&message);
    assert!(parsed.is_ok(), "Message should parse");

    // 2. Extract command parameters (simulating server's handle_tools_call)
    if let Ok(IncomingMessage::Request(req)) = parsed {
        let params = req.params.expect("Should have params");
        let arguments = &params["arguments"];

        let command = arguments["command"].as_str().unwrap();
        let args: Vec<String> = arguments["args"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(Value::as_str)
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();
        let cwd = arguments["cwd"].as_str().map(PathBuf::from);

        // 3. Validate the command
        let git_command = git_proxy_mcp::git::command::GitCommand::new(command, args, cwd);
        assert!(git_command.is_ok(), "Command should be valid");

        // 4. Execute via GitExecutor
        let executor = GitExecutor::new();
        let result = executor.execute(&git_command.unwrap()).await;

        // 5. Verify result
        assert!(result.is_ok(), "Execution should succeed");
        let output = result.unwrap();
        assert!(output.success, "Fetch should succeed");

        // 6. Verify output is sanitised
        assert!(
            !output.stdout.contains("ghp_"),
            "Output should not contain tokens"
        );
        assert!(
            !output.stderr.contains("glpat-"),
            "Stderr should not contain tokens"
        );
    }
}

#[tokio::test]
async fn test_pipeline_blocks_dangerous_command() {
    // This test verifies the full validation chain

    // 1. Parse a message with a dangerous command
    let message = json!({
        "jsonrpc": "2.0",
        "id": 6,
        "method": "tools/call",
        "params": {
            "name": "git",
            "arguments": {
                "command": "config",  // Not in allowed list
                "args": ["--global", "user.email"]
            }
        }
    })
    .to_string();

    let parsed = parse_message(&message);
    assert!(parsed.is_ok(), "Message should parse (protocol level)");

    if let Ok(IncomingMessage::Request(req)) = parsed {
        let params = req.params.expect("Should have params");
        let command = params["arguments"]["command"].as_str().unwrap();

        // 2. Command validation should fail
        let result = git_proxy_mcp::git::command::GitCommand::new(
            command,
            vec!["--global".to_string(), "user.email".to_string()],
            None,
        );

        assert!(result.is_err(), "config command should be rejected");
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("not allowed"),
            "Error should mention command not allowed"
        );
    }
}

#[test]
fn test_pipeline_security_guard_blocks_force_push() {
    use git_proxy_mcp::security::guards::{PushGuard, SecurityGuard};

    // Default security config blocks force push
    let guard = PushGuard::default();

    // Simulate a force push command that would come through the pipeline
    let command = "push";
    let args = vec![
        "--force".to_string(),
        "origin".to_string(),
        "main".to_string(),
    ];

    let result = guard.check(command, &args);
    assert!(result.is_blocked(), "Force push should be blocked");
    assert!(
        result.reason().is_some(),
        "Should have a reason for blocking"
    );
}

// =============================================================================
// Concurrent Request Tests
// =============================================================================

#[tokio::test]
async fn test_concurrent_command_execution() {
    if !git_available() {
        eprintln!("Skipping test: git not available");
        return;
    }

    // Create temp repos with their bare remotes
    let Some(repo1) = create_temp_repo() else {
        eprintln!("Skipping test: failed to create temp repo 1");
        return;
    };
    let Some(repo2) = create_temp_repo() else {
        eprintln!("Skipping test: failed to create temp repo 2");
        return;
    };

    let Some(bare1) = create_bare_repo() else {
        eprintln!("Skipping test: failed to create bare repo 1");
        return;
    };
    let Some(bare2) = create_bare_repo() else {
        eprintln!("Skipping test: failed to create bare repo 2");
        return;
    };

    // Add remotes
    Command::new("git")
        .args(["remote", "add", "origin", bare1.path().to_str().unwrap()])
        .current_dir(repo1.path())
        .output()
        .expect("Should add remote 1");

    Command::new("git")
        .args(["remote", "add", "origin", bare2.path().to_str().unwrap()])
        .current_dir(repo2.path())
        .output()
        .expect("Should add remote 2");

    // Create fetch commands
    let cmd1 = git_proxy_mcp::git::command::GitCommand::new(
        "fetch",
        vec!["origin".to_string()],
        Some(repo1.path().to_path_buf()),
    )
    .expect("fetch command should be valid");

    let cmd2 = git_proxy_mcp::git::command::GitCommand::new(
        "fetch",
        vec!["origin".to_string()],
        Some(repo2.path().to_path_buf()),
    )
    .expect("fetch command should be valid");

    let executor1 = GitExecutor::new();
    let executor2 = GitExecutor::new();

    // Execute fetch commands concurrently using tokio::join!
    let (result1, result2) = tokio::join!(executor1.execute(&cmd1), executor2.execute(&cmd2));

    // Both should succeed
    assert!(result1.is_ok(), "Concurrent fetch 1 should succeed");
    assert!(result2.is_ok(), "Concurrent fetch 2 should succeed");
    assert!(
        result1.unwrap().success,
        "Concurrent fetch 1 should have exit code 0"
    );
    assert!(
        result2.unwrap().success,
        "Concurrent fetch 2 should have exit code 0"
    );
}

// =============================================================================
// Error Handling Integration Tests
// =============================================================================

#[tokio::test]
async fn test_executor_handles_git_command_failure() {
    if !git_available() {
        eprintln!("Skipping test: git not available");
        return;
    }

    let Some(repo_dir) = create_temp_repo() else {
        eprintln!("Skipping test: failed to create temp repo");
        return;
    };

    let executor = GitExecutor::new();

    // Try to fetch from a non-existent remote
    let command = git_proxy_mcp::git::command::GitCommand::new(
        "fetch",
        vec!["nonexistent-remote".to_string()],
        Some(repo_dir.path().to_path_buf()),
    )
    .expect("fetch command should be valid");

    let result = executor.execute(&command).await;

    // Command should execute (not error), but git should fail
    assert!(result.is_ok(), "Executor should not error");
    let output = result.unwrap();
    assert!(!output.success, "Git should fail for non-existent remote");
    assert!(
        output
            .stderr
            .contains("does not appear to be a git repository")
            || output.stderr.contains("No such remote"),
        "Error should mention the remote issue"
    );
}

#[test]
fn test_protocol_error_handling() {
    // Test various malformed messages

    // Missing jsonrpc version
    let result = parse_message(r#"{"id": 1, "method": "test"}"#);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.error.code, -32600); // Invalid request

    // Invalid JSON
    let result = parse_message("not json at all");
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.error.code, -32700); // Parse error

    // Wrong jsonrpc version
    let result = parse_message(r#"{"jsonrpc": "1.0", "id": 1, "method": "test"}"#);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.error.code, -32600); // Invalid request
}
