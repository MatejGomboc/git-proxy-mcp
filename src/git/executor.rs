//! Git command execution.
//!
//! This module handles:
//!
//! 1. Executing Git as a subprocess
//! 2. Capturing and sanitising output
//! 3. Detecting Git LFS usage
//! 4. Enforcing execution timeouts
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
use std::time::Duration;

use tokio::process::Command;
use tokio::time::timeout;

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

    /// Whether stdout was truncated due to size limits.
    pub stdout_truncated: bool,

    /// Whether stderr was truncated due to size limits.
    pub stderr_truncated: bool,
}

impl CommandOutput {
    /// Creates a new command output with default (no truncation) flags.
    ///
    /// This is a convenience constructor for creating non-truncated outputs.
    #[cfg(test)]
    const fn new(stdout: String, stderr: String, exit_code: i32) -> Self {
        Self {
            stdout,
            stderr,
            exit_code,
            success: exit_code == 0,
            warnings: Vec::new(),
            stdout_truncated: false,
            stderr_truncated: false,
        }
    }

    /// Creates a new command output with truncation flags.
    #[must_use]
    pub const fn new_with_truncation(
        stdout: String,
        stderr: String,
        exit_code: i32,
        stdout_truncated: bool,
        stderr_truncated: bool,
    ) -> Self {
        Self {
            stdout,
            stderr,
            exit_code,
            success: exit_code == 0,
            warnings: Vec::new(),
            stdout_truncated,
            stderr_truncated,
        }
    }

    /// Adds a warning message.
    fn add_warning(&mut self, warning: impl Into<String>) {
        self.warnings.push(warning.into());
    }

    /// Returns whether any output was truncated.
    #[must_use]
    pub const fn is_truncated(&self) -> bool {
        self.stdout_truncated || self.stderr_truncated
    }
}

/// Default timeout for git command execution (5 minutes).
const DEFAULT_TIMEOUT_SECS: u64 = 300;

/// Default maximum output size in bytes (10 MiB).
const DEFAULT_MAX_OUTPUT_BYTES: usize = 10 * 1024 * 1024;

/// Executes Git commands as subprocesses.
///
/// This executor spawns git commands using the user's existing Git
/// configuration. It does not store or inject credentials — authentication
/// is handled by the user's credential helpers and SSH agent.
pub struct GitExecutor {
    /// Output sanitiser for removing credentials from output.
    sanitiser: OutputSanitiser,

    /// Timeout for git command execution.
    timeout: Duration,

    /// Maximum output size in bytes (combined stdout + stderr).
    max_output_bytes: usize,
}

impl Default for GitExecutor {
    fn default() -> Self {
        Self::new()
    }
}

impl GitExecutor {
    /// Creates a new Git executor with default settings.
    #[must_use]
    pub fn new() -> Self {
        Self {
            sanitiser: OutputSanitiser::new(),
            timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECS),
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
        }
    }

    /// Creates a new Git executor with a custom timeout.
    #[must_use]
    pub fn with_timeout(timeout: Duration) -> Self {
        Self {
            sanitiser: OutputSanitiser::new(),
            timeout,
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
        }
    }

    /// Creates a new Git executor with custom timeout and output size limit.
    #[must_use]
    pub fn with_limits(timeout: Duration, max_output_bytes: usize) -> Self {
        Self {
            sanitiser: OutputSanitiser::new(),
            timeout,
            max_output_bytes,
        }
    }

    /// Returns the configured timeout duration.
    #[must_use]
    pub const fn timeout(&self) -> Duration {
        self.timeout
    }

    /// Returns the configured maximum output size in bytes.
    #[must_use]
    pub const fn max_output_bytes(&self) -> usize {
        self.max_output_bytes
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
    /// - The command execution times out
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

        // Execute the command with timeout
        let output = timeout(self.timeout, cmd.output())
            .await
            .map_err(|_| ExecutorError::Timeout {
                timeout_secs: self.timeout.as_secs(),
            })?
            .map_err(|e| ExecutorError::ProcessError {
                message: format!("Failed to execute git: {e}"),
            })?;

        // Convert output to strings and sanitise
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        let sanitised_stdout = self.sanitiser.sanitise(&stdout).into_owned();
        let sanitised_stderr = self.sanitiser.sanitise(&stderr).into_owned();

        // Apply output size limits
        let (final_stdout, stdout_truncated) =
            Self::truncate_output(&sanitised_stdout, self.max_output_bytes);
        let remaining_budget = self.max_output_bytes.saturating_sub(final_stdout.len());
        let (final_stderr, stderr_truncated) =
            Self::truncate_output(&sanitised_stderr, remaining_budget);

        let exit_code = output.status.code().unwrap_or(-1);

        let mut result = CommandOutput::new_with_truncation(
            final_stdout,
            final_stderr,
            exit_code,
            stdout_truncated,
            stderr_truncated,
        );

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

    /// Truncates output to the specified maximum byte length.
    ///
    /// Returns the (possibly truncated) string and a boolean indicating
    /// whether truncation occurred. Truncation is done at a UTF-8 character
    /// boundary to avoid invalid UTF-8 sequences.
    fn truncate_output(output: &str, max_bytes: usize) -> (String, bool) {
        if output.len() <= max_bytes {
            return (output.to_string(), false);
        }

        // Find the last character that fits entirely within max_bytes
        let truncate_at = output
            .char_indices()
            .take_while(|(idx, c)| idx + c.len_utf8() <= max_bytes)
            .last()
            .map_or(0, |(idx, c)| idx + c.len_utf8());

        (output[..truncate_at].to_string(), true)
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

    /// Command execution timed out.
    #[error("command timed out after {timeout_secs} seconds")]
    Timeout {
        /// Timeout duration in seconds.
        timeout_secs: u64,
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
        assert_eq!(
            executor.timeout(),
            Duration::from_secs(DEFAULT_TIMEOUT_SECS)
        );
    }

    #[test]
    fn executor_with_custom_timeout() {
        let timeout = Duration::from_secs(60);
        let executor = GitExecutor::with_timeout(timeout);
        assert_eq!(executor.timeout(), timeout);
        assert_eq!(executor.max_output_bytes(), DEFAULT_MAX_OUTPUT_BYTES);
    }

    #[test]
    fn executor_with_limits() {
        let timeout = Duration::from_secs(60);
        let max_output = 1024 * 1024; // 1 MiB
        let executor = GitExecutor::with_limits(timeout, max_output);
        assert_eq!(executor.timeout(), timeout);
        assert_eq!(executor.max_output_bytes(), max_output);
    }

    #[test]
    fn timeout_error_display() {
        let error = ExecutorError::Timeout { timeout_secs: 300 };
        let msg = error.to_string();
        assert!(msg.contains("timed out"));
        assert!(msg.contains("300 seconds"));
    }

    #[test]
    fn truncate_output_no_truncation() {
        let output = "Hello, world!";
        let (result, truncated) = GitExecutor::truncate_output(output, 100);
        assert_eq!(result, "Hello, world!");
        assert!(!truncated);
    }

    #[test]
    fn truncate_output_exact_limit() {
        let output = "Hello";
        let (result, truncated) = GitExecutor::truncate_output(output, 5);
        assert_eq!(result, "Hello");
        assert!(!truncated);
    }

    #[test]
    fn truncate_output_basic() {
        let output = "Hello, world!";
        let (result, truncated) = GitExecutor::truncate_output(output, 5);
        assert_eq!(result, "Hello");
        assert!(truncated);
    }

    #[test]
    fn truncate_output_utf8_boundary() {
        // Test with multi-byte UTF-8 characters (emoji: 4 bytes each)
        let output = "Hi 👋🌍"; // "Hi " = 3 bytes, emoji = 4 bytes each = 11 bytes total

        // With limit 11, fits exactly (no truncation)
        let (result, truncated) = GitExecutor::truncate_output(output, 11);
        assert_eq!(result, "Hi 👋🌍");
        assert!(!truncated);

        // With limit 7, truncates to "Hi 👋" (3 + 4 = 7 bytes)
        let (result, truncated) = GitExecutor::truncate_output(output, 7);
        assert_eq!(result, "Hi 👋");
        assert!(truncated);

        // With limit 6, can't fit the emoji (4 bytes), so only "Hi "
        let (result, truncated) = GitExecutor::truncate_output(output, 6);
        assert_eq!(result, "Hi ");
        assert!(truncated);
    }

    #[test]
    fn truncate_output_multibyte_char_boundary() {
        // "日本語" = 9 bytes (3 bytes per character)
        let output = "日本語";

        // With limit 9, fits exactly (no truncation)
        let (result, truncated) = GitExecutor::truncate_output(output, 9);
        assert_eq!(result, "日本語");
        assert!(!truncated);

        // With limit 6, truncates to "日本" (6 bytes)
        let (result, truncated) = GitExecutor::truncate_output(output, 6);
        assert_eq!(result, "日本");
        assert!(truncated);

        // With limit 5, can only fit first character (3 bytes)
        let (result, truncated) = GitExecutor::truncate_output(output, 5);
        assert_eq!(result, "日");
        assert!(truncated);
    }

    #[test]
    fn truncate_output_empty() {
        let output = "";
        let (result, truncated) = GitExecutor::truncate_output(output, 10);
        assert_eq!(result, "");
        assert!(!truncated);
    }

    #[test]
    fn truncate_output_zero_limit() {
        let output = "Hello";
        let (result, truncated) = GitExecutor::truncate_output(output, 0);
        assert_eq!(result, "");
        assert!(truncated);
    }

    #[test]
    fn command_output_truncation_flags() {
        let output = CommandOutput::new_with_truncation(
            "stdout".to_string(),
            "stderr".to_string(),
            0,
            true,
            false,
        );
        assert!(output.stdout_truncated);
        assert!(!output.stderr_truncated);
        assert!(output.is_truncated());

        let output = CommandOutput::new_with_truncation(
            "stdout".to_string(),
            "stderr".to_string(),
            0,
            false,
            false,
        );
        assert!(!output.is_truncated());
    }
}
