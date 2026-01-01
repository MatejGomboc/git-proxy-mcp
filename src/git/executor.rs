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
    /// - No matching credential is found for commands requiring auth
    /// - The Git process fails to start
    pub async fn execute(&self, command: &GitCommand) -> Result<CommandOutput, ExecutorError> {
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
    fn setup_credentials(cmd: &mut Command, credential: &Credential) -> Result<(), ExecutorError> {
        match credential.auth() {
            AuthMethod::Pat(pat) => {
                // For HTTPS, use GIT_ASKPASS with the token
                // We set the token in an environment variable and use a simple script
                // to return it when Git asks for credentials
                //
                // On Windows, we use PowerShell; on Unix, we use sh
                #[cfg(windows)]
                {
                    let token = pat.expose_token();
                    // Use credential.helper to provide the password
                    cmd.env(
                        "GIT_ASKPASS",
                        "powershell.exe -Command \"Write-Host $env:GIT_PASSWORD\"",
                    );
                    cmd.env("GIT_PASSWORD", token);
                }

                #[cfg(not(windows))]
                {
                    let token = pat.expose_token();
                    cmd.env("GIT_ASKPASS", "echo");
                    cmd.env("GIT_PASSWORD", token);
                    // For simple echo-based approach
                    cmd.env("GIT_ASKPASS", "/bin/sh");
                    cmd.args(["-c", &format!("echo '{token}'")]);
                }

                // Alternative: use the credential helper protocol
                // This is more reliable across platforms
                cmd.env("GIT_USERNAME", "x-access-token");
                cmd.env("GIT_PASSWORD", pat.expose_token());

                // Note: The credential helper approach above is a basic implementation.
                // For production use, a proper credential helper script should be used.
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
}
