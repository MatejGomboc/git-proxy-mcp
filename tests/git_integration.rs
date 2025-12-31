//! Integration tests for Git command proxy functionality.
//!
//! These tests verify the complete Git command execution pipeline,
//! including command validation and output sanitisation.

use std::path::PathBuf;
use std::process::Command;

use tempfile::TempDir;

use git_proxy_mcp::git::command::GitCommand;

/// Helper to check if git is available on the system.
fn git_available() -> bool {
    Command::new("git").arg("--version").output().is_ok()
}

/// Helper to create a temporary git repository for testing.
fn create_temp_repo() -> Option<TempDir> {
    let temp_dir = TempDir::new().ok()?;

    // Initialize a git repository
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

/// Helper to create a file in the temp repo.
fn create_file(temp_dir: &TempDir, name: &str, content: &str) -> std::io::Result<()> {
    let path = temp_dir.path().join(name);
    std::fs::write(path, content)
}

// =============================================================================
// Command Validation Tests
// =============================================================================

#[test]
fn test_allowed_commands_accepted() {
    let allowed = [
        "add",
        "branch",
        "checkout",
        "clone",
        "commit",
        "diff",
        "fetch",
        "init",
        "log",
        "ls-files",
        "ls-remote",
        "merge",
        "pull",
        "push",
        "rebase",
        "remote",
        "reset",
        "rev-parse",
        "revert",
        "show",
        "stash",
        "status",
        "tag",
    ];

    for cmd in allowed {
        let result = GitCommand::new(cmd, vec![], None);
        assert!(result.is_ok(), "Command '{cmd}' should be allowed");
    }
}

#[test]
fn test_dangerous_commands_rejected() {
    // Commands not in the allowlist
    let dangerous = ["config", "gc", "filter-branch", "reflog", "fsck", "prune"];

    for cmd in dangerous {
        let result = GitCommand::new(cmd, vec![], None);
        assert!(result.is_err(), "Command '{cmd}' should be rejected");
    }
}

#[test]
fn test_dangerous_flags_rejected() {
    let dangerous_flags = [
        "--exec=malicious",
        "-c",
        "--upload-pack",
        "--receive-pack",
        "--no-verify",
        "--verbose",
        "-v",
        "--debug",
        "--git-dir=/etc",
        "--work-tree=/tmp",
    ];

    for flag in dangerous_flags {
        let result = GitCommand::new("status", vec![flag.to_string()], None);
        assert!(result.is_err(), "Flag '{flag}' should be rejected");
    }
}

#[test]
fn test_relative_working_dir_rejected() {
    let result = GitCommand::new("status", vec![], Some(PathBuf::from("./relative")));
    assert!(
        result.is_err(),
        "Relative working directory should be rejected"
    );
}

#[test]
fn test_absolute_working_dir_accepted() {
    #[cfg(windows)]
    let path = PathBuf::from("C:\\Users\\test");
    #[cfg(not(windows))]
    let path = PathBuf::from("/home/test");

    let result = GitCommand::new("status", vec![], Some(path));
    assert!(
        result.is_ok(),
        "Absolute working directory should be accepted"
    );
}

// =============================================================================
// Git System Tests (Local Operations using system git)
// =============================================================================

#[test]
fn test_git_init_creates_repo() {
    if !git_available() {
        eprintln!("Skipping test: git not available");
        return;
    }

    let temp_dir = match TempDir::new() {
        Ok(dir) => dir,
        Err(_) => {
            eprintln!("Skipping test: failed to create temp dir");
            return;
        }
    };

    let output = Command::new("git")
        .args(["init"])
        .current_dir(temp_dir.path())
        .output()
        .expect("git init should succeed");

    assert!(output.status.success());
    assert!(temp_dir.path().join(".git").exists());
}

#[test]
fn test_git_status_in_repo() {
    if !git_available() {
        eprintln!("Skipping test: git not available");
        return;
    }

    let temp_dir = match create_temp_repo() {
        Some(dir) => dir,
        None => {
            eprintln!("Skipping test: failed to create temp repo");
            return;
        }
    };

    let output = Command::new("git")
        .args(["status"])
        .current_dir(temp_dir.path())
        .output()
        .expect("git status should succeed");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("On branch") || stdout.contains("No commits yet"));
}

#[test]
fn test_git_add_and_commit() {
    if !git_available() {
        eprintln!("Skipping test: git not available");
        return;
    }

    let temp_dir = match create_temp_repo() {
        Some(dir) => dir,
        None => {
            eprintln!("Skipping test: failed to create temp repo");
            return;
        }
    };

    // Create a test file
    create_file(&temp_dir, "test.txt", "Hello, World!").expect("Failed to create test file");

    // Test git add
    let add_output = Command::new("git")
        .args(["add", "test.txt"])
        .current_dir(temp_dir.path())
        .output()
        .expect("git add should succeed");
    assert!(add_output.status.success());

    // Test git commit
    let commit_output = Command::new("git")
        .args(["commit", "-m", "Initial commit"])
        .current_dir(temp_dir.path())
        .output()
        .expect("git commit should succeed");
    assert!(commit_output.status.success());
}

#[test]
fn test_git_log() {
    if !git_available() {
        eprintln!("Skipping test: git not available");
        return;
    }

    let temp_dir = match create_temp_repo() {
        Some(dir) => dir,
        None => {
            eprintln!("Skipping test: failed to create temp repo");
            return;
        }
    };

    // Create and commit a file
    create_file(&temp_dir, "test.txt", "content").expect("Failed to create file");

    Command::new("git")
        .args(["add", "."])
        .current_dir(temp_dir.path())
        .output()
        .expect("git add failed");

    Command::new("git")
        .args(["commit", "-m", "Test commit"])
        .current_dir(temp_dir.path())
        .output()
        .expect("git commit failed");

    let log_output = Command::new("git")
        .args(["log", "--oneline"])
        .current_dir(temp_dir.path())
        .output()
        .expect("git log should succeed");

    assert!(log_output.status.success());
    let stdout = String::from_utf8_lossy(&log_output.stdout);
    assert!(stdout.contains("Test commit"));
}

#[test]
fn test_git_branch_operations() {
    if !git_available() {
        eprintln!("Skipping test: git not available");
        return;
    }

    let temp_dir = match create_temp_repo() {
        Some(dir) => dir,
        None => {
            eprintln!("Skipping test: failed to create temp repo");
            return;
        }
    };

    // Need at least one commit to create branches
    create_file(&temp_dir, "test.txt", "content").expect("Failed to create file");

    Command::new("git")
        .args(["add", "."])
        .current_dir(temp_dir.path())
        .output()
        .expect("git add failed");

    Command::new("git")
        .args(["commit", "-m", "Initial commit"])
        .current_dir(temp_dir.path())
        .output()
        .expect("git commit failed");

    // Create a new branch
    let branch_output = Command::new("git")
        .args(["branch", "feature-branch"])
        .current_dir(temp_dir.path())
        .output()
        .expect("git branch should succeed");
    assert!(branch_output.status.success());

    // List branches
    let list_output = Command::new("git")
        .args(["branch"])
        .current_dir(temp_dir.path())
        .output()
        .expect("git branch list should succeed");

    assert!(list_output.status.success());
    let stdout = String::from_utf8_lossy(&list_output.stdout);
    assert!(stdout.contains("feature-branch"));
}

#[test]
fn test_git_diff() {
    if !git_available() {
        eprintln!("Skipping test: git not available");
        return;
    }

    let temp_dir = match create_temp_repo() {
        Some(dir) => dir,
        None => {
            eprintln!("Skipping test: failed to create temp repo");
            return;
        }
    };

    // Create and commit a file
    create_file(&temp_dir, "test.txt", "original").expect("Failed to create file");

    Command::new("git")
        .args(["add", "."])
        .current_dir(temp_dir.path())
        .output()
        .expect("git add failed");

    Command::new("git")
        .args(["commit", "-m", "Initial"])
        .current_dir(temp_dir.path())
        .output()
        .expect("git commit failed");

    // Modify the file
    create_file(&temp_dir, "test.txt", "modified").expect("Failed to modify file");

    let diff_output = Command::new("git")
        .args(["diff"])
        .current_dir(temp_dir.path())
        .output()
        .expect("git diff should succeed");

    assert!(diff_output.status.success());
    let stdout = String::from_utf8_lossy(&diff_output.stdout);
    assert!(stdout.contains("-original") || stdout.contains("+modified"));
}

// =============================================================================
// Command Building Tests
// =============================================================================

#[test]
fn test_build_args_includes_command() {
    let cmd = GitCommand::new(
        "commit",
        vec!["-m".to_string(), "message".to_string()],
        None,
    )
    .expect("commit command should be valid");

    let args = cmd.build_args();
    assert_eq!(args[0], "commit");
    assert_eq!(args[1], "-m");
    assert_eq!(args[2], "message");
}

#[test]
fn test_requires_auth_detection() {
    let auth_commands = ["clone", "push", "pull", "fetch", "ls-remote"];
    for cmd_name in auth_commands {
        let cmd = GitCommand::new(cmd_name, vec![], None).expect("command should be valid");
        assert!(cmd.requires_auth(), "{cmd_name} should require auth");
    }

    let no_auth_commands = ["status", "log", "diff", "branch", "add", "commit"];
    for cmd_name in no_auth_commands {
        let cmd = GitCommand::new(cmd_name, vec![], None).expect("command should be valid");
        assert!(!cmd.requires_auth(), "{cmd_name} should not require auth");
    }
}

#[test]
fn test_extract_remote_url() {
    // Clone command
    let clone_cmd = GitCommand::new(
        "clone",
        vec!["https://github.com/user/repo.git".to_string()],
        None,
    )
    .expect("clone command should be valid");
    assert_eq!(
        clone_cmd.extract_remote_url(),
        Some("https://github.com/user/repo.git")
    );

    // Push command
    let push_cmd = GitCommand::new("push", vec!["origin".to_string(), "main".to_string()], None)
        .expect("push command should be valid");
    assert_eq!(push_cmd.extract_remote_url(), Some("origin"));

    // Status command (no remote)
    let status_cmd =
        GitCommand::new("status", vec![], None).expect("status command should be valid");
    assert!(status_cmd.extract_remote_url().is_none());
}
