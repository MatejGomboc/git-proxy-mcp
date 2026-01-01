//! Git command execution.
//!
//! This module handles:
//!
//! 1. Executing Git as a subprocess
//! 2. Capturing and sanitising output
//! 3. Detecting Git LFS usage
//!
//! # Credential Handling
//!
//! This executor does NOT handle credentials directly. Instead, it relies
//! on the user's existing Git configuration:
//!
//! - Credential helpers (macOS Keychain, Windows Credential Manager, libsecret)
//! - SSH agent for SSH key authentication
//! - `GIT_TERMINAL_PROMPT=0` prevents interactive credential prompts

use std::process::Stdio;

use tokio::process::Command;

use crate::git::command::GitCommand;
use crate::git::sanitiser::OutputSanitiser;

/// Output from a Git command execution.
#[derive(Debug, Clone)]
pub struct CommandOutput {
    /// Standard output (sanitised).
    pub stdout: String,

    /// Standard error (sanitised).
    pub stderr: String,

    /// Exit code from the process.
    pub exit_code: i32,

    /// Whether the command succeeded (exit code 0).
    pub success: bool,

    /// Warning messages (e.g., LFS detected).
    pub warnings: Vec<String>,
}

impl CommandOutput {
    /// Creates a new command output.
    const fn new(stdout: String, stderr: String, exit_code: i32) -> Self {
        Self {
            stdout,
            stderr,
            exit_code,
            success: exit_code == 0,
            warnings: Vec::new(),
        }
    }

    /// Adds a warning message.
    fn add_warning(&mut self, warning: impl Into<String>) {
        self.warnings.push(warning.into());
    }
}

/// Executes Git commands as subprocesses.
///
/// This executor spawns git commands using the user's existing Git
/// configuration. It does not store or inject credentials — authentication
/// is handled by the user's credential helpers and SSH agent.
pub struct GitExecutor {
    /// Output sanitiser for removing credentials from output.
    sanitiser: OutputSanitiser,
}

impl Default for GitExecutor {
    fn default() -> Self {
        Self::new()
    }
}

impl GitExecutor {
    /// Creates a new Git executor.
    #[must_use]
    pub fn new() -> Self {
        Self {
            sanitiser: OutputSanitiser::new(),
        }
    }

    /// Executes a Git command.
    ///
    /// # Arguments
    ///
    /// * `command` — The validated Git command to execute
    ///
    /// # Returns
    ///
    /// The command output with stdout, stderr, and exit code.
    /// All output is sanitised to remove potential credential leaks.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The working directory does not exist or is not accessible
    /// - The Git process fails to start
    pub async fn execute(&self, command: &GitCommand) -> Result<CommandOutput, ExecutorError> {
        // Validate working directory exists before executing
        if let Some(dir) = command.working_dir() {
            Self::validate_working_directory(dir)?;
        }

        // Build the command
        let mut cmd = Command::new("git");

        // Set working directory if specified
        if let Some(dir) = command.working_dir() {
            cmd.current_dir(dir);
        }

        // Add command and arguments
        cmd.args(command.build_args());

        // Configure stdio
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        // Prevent Git from prompting for credentials interactively.
        // If credentials are not available via credential helpers or SSH agent,
        // git will fail with an error rather than hanging.
        cmd.env("GIT_TERMINAL_PROMPT", "0");

        // Execute the command
        let output = cmd
            .output()
            .await
            .map_err(|e| ExecutorError::ProcessError {
                message: format!("Failed to execute git: {e}"),
            })?;

        // Convert output to strings and sanitise
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        let sanitised_stdout = self.sanitiser.sanitise(&stdout).into_owned();
        let sanitised_stderr = self.sanitiser.sanitise(&stderr).into_owned();

        let exit_code = output.status.code().unwrap_or(-1);

        let mut result = CommandOutput::new(sanitised_stdout, sanitised_stderr, exit_code);

        // Check for LFS
        if Self::detect_lfs(&result) {
            result.add_warning(
                "Git LFS objects detected. LFS support is not yet implemented. \
                 Large files may not be downloaded correctly.",
            );
        }

        Ok(result)
    }

    /// Detects if the output indicates Git LFS usage.
    fn detect_lfs(output: &CommandOutput) -> bool {
        let lfs_indicators = [
            "git-lfs",
            "lfs.fetchinclude",
            "lfs.fetchexclude",
            "filter=lfs",
            "Downloading LFS",
            "LFS object",
            ".gitattributes: filter=lfs",
        ];

        for indicator in &lfs_indicators {
            if output.stdout.contains(indicator) || output.stderr.contains(indicator) {
                return true;
            }
        }

        false
    }

    /// Validates that a working directory exists and is accessible.
    ///
    /// # Errors
    ///
    /// Returns an error if the directory:
    /// - Does not exist
    /// - Is not a directory
    /// - Cannot be accessed
    fn validate_working_directory(dir: &std::path::Path) -> Result<(), ExecutorError> {
        if !dir.exists() {
            return Err(ExecutorError::WorkingDirectoryError {
                message: format!("directory does not exist: {}", dir.display()),
            });
        }

        if !dir.is_dir() {
            return Err(ExecutorError::WorkingDirectoryError {
                message: format!("path is not a directory: {}", dir.display()),
            });
        }

        // Check if we can read the directory (access check)
        if std::fs::read_dir(dir).is_err() {
            return Err(ExecutorError::WorkingDirectoryError {
                message: format!("cannot access directory: {}", dir.display()),
            });
        }

        Ok(())
    }
}

/// Errors that can occur during Git command execution.
#[derive(Debug, thiserror::Error)]
pub enum ExecutorError {
    /// Failed to execute the Git process.
    #[error("process error: {message}")]
    ProcessError {
        /// Error message.
        message: String,
    },

    /// Working directory does not exist or is not accessible.
    #[error("working directory error: {message}")]
    WorkingDirectoryError {
        /// Description of the error.
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_output_success() {
        let output = CommandOutput::new("output".to_string(), String::new(), 0);
        assert!(output.success);
        assert_eq!(output.exit_code, 0);
    }

    #[test]
    fn command_output_failure() {
        let output = CommandOutput::new(String::new(), "error".to_string(), 1);
        assert!(!output.success);
        assert_eq!(output.exit_code, 1);
    }

    #[test]
    fn command_output_warnings() {
        let mut output = CommandOutput::new(String::new(), String::new(), 0);
        output.add_warning("Test warning");
        assert_eq!(output.warnings.len(), 1);
        assert_eq!(output.warnings[0], "Test warning");
    }

    #[test]
    fn detect_lfs_in_output() {
        let output = CommandOutput::new(
            "Downloading LFS objects: 100% (5/5)".to_string(),
            String::new(),
            0,
        );
        assert!(GitExecutor::detect_lfs(&output));

        let output = CommandOutput::new("Cloning into 'repo'...".to_string(), String::new(), 0);
        assert!(!GitExecutor::detect_lfs(&output));
    }

    #[test]
    fn validate_working_directory_exists() {
        // Current directory should always exist
        let current_dir = std::env::current_dir().unwrap();
        assert!(GitExecutor::validate_working_directory(&current_dir).is_ok());
    }

    #[test]
    fn validate_working_directory_not_exists() {
        let non_existent = std::path::PathBuf::from("/this/path/should/not/exist/anywhere");
        let result = GitExecutor::validate_working_directory(&non_existent);
        assert!(matches!(
            result,
            Err(ExecutorError::WorkingDirectoryError { .. })
        ));
    }

    #[test]
    fn validate_working_directory_is_file() {
        // Create a temporary file and try to use it as a working directory
        let temp_file = std::env::temp_dir().join("git_proxy_test_file.tmp");
        std::fs::write(&temp_file, "test").unwrap();

        let result = GitExecutor::validate_working_directory(&temp_file);
        assert!(matches!(
            result,
            Err(ExecutorError::WorkingDirectoryError { .. })
        ));

        // Clean up
        let _ = std::fs::remove_file(&temp_file);
    }

    #[test]
    fn executor_default() {
        let executor = GitExecutor::default();
        // Just verify it doesn't panic
        drop(executor);
    }
}
