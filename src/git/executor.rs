//! Git command execution with credential injection.
//!
//! This module handles:
//!
//! 1. Setting up credential helpers via environment variables
//! 2. Executing Git as a subprocess
//! 3. Capturing and sanitising output
//! 4. Detecting Git LFS usage

use std::process::Stdio;

use tokio::process::Command;

use crate::auth::{AuthMethod, Credential, CredentialStore};
use crate::error::AuthError;
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

/// Executes Git commands with credential injection.
pub struct GitExecutor {
    /// Credential store for finding matching credentials.
    credential_store: CredentialStore,

    /// Output sanitiser for removing credentials from output.
    sanitiser: OutputSanitiser,
}

impl GitExecutor {
    /// Creates a new Git executor.
    #[must_use]
    pub fn new(credential_store: CredentialStore) -> Self {
        Self {
            credential_store,
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
    /// - No matching credential is found for commands requiring auth
    /// - The Git process fails to start
    pub async fn execute(&self, command: &GitCommand) -> Result<CommandOutput, ExecutorError> {
        // Validate working directory exists before executing
        if let Some(dir) = command.working_dir() {
            Self::validate_working_directory(dir)?;
        }

        // Find credentials if needed
        let credential = if command.requires_auth() {
            if let Some(url) = command.extract_remote_url() {
                // Try to find a matching credential
                match self.credential_store.find_credential(url) {
                    Ok(cred) => Some(cred),
                    Err(AuthError::NoMatchingCredential { .. }) => {
                        // For some remotes (like "origin"), we need to resolve the actual URL
                        // This will be handled in a future phase
                        None
                    }
                    Err(e) => return Err(ExecutorError::CredentialError(e)),
                }
            } else {
                None
            }
        } else {
            None
        };

        // Build the command
        let mut cmd = Command::new("git");

        // Set working directory if specified
        if let Some(dir) = command.working_dir() {
            cmd.current_dir(dir);
        }

        // Add command and arguments
        cmd.args(command.build_args());

        // Set up credential injection
        if let Some(cred) = credential {
            Self::setup_credentials(&mut cmd, cred)?;
        }

        // Configure stdio
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        // Prevent Git from prompting for credentials
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

    /// Sets up credential injection for a command.
    ///
    /// # Security
    ///
    /// Credentials are passed via environment variables, never via command-line
    /// arguments, to prevent exposure in process listings (`ps`, `/proc`, etc.).
    fn setup_credentials(cmd: &mut Command, credential: &Credential) -> Result<(), ExecutorError> {
        match credential.auth() {
            AuthMethod::Pat(pat) => {
                // For HTTPS, we use GIT_ASKPASS with an inline script that reads
                // the password from an environment variable. This approach:
                //
                // 1. Never exposes the token in command-line arguments
                // 2. Works on both Windows and Unix
                // 3. Uses environment variables which are not visible in `ps` output
                //
                // Git calls GIT_ASKPASS with a prompt like "Password for 'https://...': "
                // The script ignores the prompt and just prints the password.

                let token = pat.expose_token();

                #[cfg(windows)]
                {
                    // On Windows, use cmd.exe to echo the environment variable
                    // The /c flag runs the command and exits
                    cmd.env("GIT_ASKPASS", "cmd.exe");
                    cmd.env("GIT_ASKPASS_CMD", "/c echo %GIT_PROXY_TOKEN%");
                    cmd.env("GIT_PROXY_TOKEN", token);

                    // Set up credential helper config via environment
                    // This tells git to use x-access-token as username
                    cmd.env("GIT_CONFIG_COUNT", "2");
                    cmd.env("GIT_CONFIG_KEY_0", "credential.username");
                    cmd.env("GIT_CONFIG_VALUE_0", "x-access-token");
                    cmd.env("GIT_CONFIG_KEY_1", "credential.helper");
                    cmd.env(
                        "GIT_CONFIG_VALUE_1",
                        "!cmd.exe /c echo password=%GIT_PROXY_TOKEN%",
                    );
                }

                #[cfg(not(windows))]
                {
                    // On Unix, we use a simple shell script via GIT_ASKPASS
                    // The script prints the token from the environment variable
                    //
                    // We use /bin/sh -c with printf to avoid echo's newline behaviour
                    // differences across platforms. The token is in an env var, not
                    // in the script text, so it won't appear in `ps` output.
                    //
                    // Note: GIT_ASKPASS is called as: $GIT_ASKPASS "prompt text"
                    // We create a script that ignores the prompt and prints the token.

                    // Store the token in an environment variable (not visible in ps)
                    cmd.env("GIT_PROXY_TOKEN", token);

                    // Use a credential helper that reads from environment
                    // This is more reliable than GIT_ASKPASS for complex scenarios
                    cmd.env("GIT_CONFIG_COUNT", "2");
                    cmd.env("GIT_CONFIG_KEY_0", "credential.username");
                    cmd.env("GIT_CONFIG_VALUE_0", "x-access-token");
                    cmd.env("GIT_CONFIG_KEY_1", "credential.helper");
                    // The credential helper outputs in git-credential format
                    // The ! prefix tells git to run it as a shell command
                    cmd.env(
                        "GIT_CONFIG_VALUE_1",
                        "!f() { echo \"password=$GIT_PROXY_TOKEN\"; }; f",
                    );
                }
            }
            AuthMethod::SshKey(ssh_key) => {
                // For SSH, set GIT_SSH_COMMAND to use the specific key
                let key_path = ssh_key.key_path();

                let ssh_cmd = format!(
                    "ssh -i {} -o IdentitiesOnly=yes -o StrictHostKeyChecking=accept-new",
                    key_path.display()
                );

                // Add passphrase handling if needed (requires ssh-add or sshpass)
                if ssh_key.expose_passphrase().is_some() {
                    // For keys with passphrases, we'd need to use ssh-add or sshpass
                    // This is complex and platform-specific, so we'll defer for now
                    return Err(ExecutorError::UnsupportedAuth {
                        reason: "SSH keys with passphrases are not yet supported. \
                                 Please use ssh-agent or a key without a passphrase."
                            .to_string(),
                    });
                }

                cmd.env("GIT_SSH_COMMAND", ssh_cmd);
            }
            AuthMethod::SshAgent(_) => {
                // SSH agent is the default behaviour, nothing to configure
                // Just ensure we're not overriding GIT_SSH_COMMAND
            }
        }

        Ok(())
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
    /// Failed to find matching credentials.
    #[error("credential error: {0}")]
    CredentialError(#[from] AuthError),

    /// Failed to execute the Git process.
    #[error("process error: {message}")]
    ProcessError {
        /// Error message.
        message: String,
    },

    /// Authentication method is not supported.
    #[error("unsupported authentication: {reason}")]
    UnsupportedAuth {
        /// Reason for the lack of support.
        reason: String,
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
}
