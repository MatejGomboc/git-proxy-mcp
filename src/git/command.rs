//! Git command parsing and validation.
//!
//! This module handles parsing incoming Git commands and validating them
//! against an allowlist of safe commands.

use std::path::PathBuf;

use thiserror::Error;

/// Errors that can occur when parsing or validating Git commands.
#[derive(Error, Debug)]
pub enum GitCommandError {
    /// The command is empty.
    #[error("git command cannot be empty")]
    EmptyCommand,

    /// The command is not in the allowlist.
    #[error("git command '{command}' is not allowed")]
    CommandNotAllowed {
        /// The disallowed command.
        command: String,
    },

    /// A dangerous flag was detected.
    #[error("dangerous flag '{flag}' is not allowed")]
    DangerousFlag {
        /// The dangerous flag.
        flag: String,
    },

    /// The working directory is invalid.
    #[error("invalid working directory: {path}")]
    InvalidWorkingDirectory {
        /// The invalid path.
        path: PathBuf,
    },
}

/// Allowlist of Git commands that can be executed.
///
/// This list is intentionally conservative to prevent misuse.
const ALLOWED_COMMANDS: &[&str] = &[
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

/// Flags that are never allowed for security reasons.
const DANGEROUS_FLAGS: &[&str] = &[
    // Arbitrary command execution
    "--exec",
    "-c", // git -c can set arbitrary config, including hooks
    "--upload-pack",
    "--receive-pack",
    // Hook manipulation
    "--no-verify", // Skip hooks (could bypass security)
    // Credential exposure risks
    "--verbose", // Some verbose modes may leak credentials
    "-v",        // Same as --verbose
    "--debug",   // Debug output may contain sensitive info
    // Path traversal
    "--git-dir", // Could access arbitrary .git directories
    "--work-tree",
];

/// A parsed and validated Git command.
#[derive(Debug, Clone)]
pub struct GitCommand {
    /// The Git subcommand (e.g., "clone", "pull").
    command: String,

    /// Arguments to pass to the command.
    args: Vec<String>,

    /// Working directory for the command (optional).
    working_dir: Option<PathBuf>,
}

impl GitCommand {
    /// Parses and validates a Git command.
    ///
    /// # Arguments
    ///
    /// * `command` — The Git subcommand (e.g., "clone", "pull")
    /// * `args` — Arguments to pass to the command
    /// * `working_dir` — Optional working directory
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The command is empty
    /// - The command is not in the allowlist
    /// - Any argument contains dangerous flags
    /// - The working directory is invalid
    pub fn new(
        command: impl Into<String>,
        args: Vec<String>,
        working_dir: Option<PathBuf>,
    ) -> Result<Self, GitCommandError> {
        let command = command.into();

        // Validate command is not empty
        if command.is_empty() {
            return Err(GitCommandError::EmptyCommand);
        }

        // Validate command is in allowlist
        if !ALLOWED_COMMANDS.contains(&command.as_str()) {
            return Err(GitCommandError::CommandNotAllowed { command });
        }

        // Check for dangerous flags in args
        for arg in &args {
            for dangerous in DANGEROUS_FLAGS {
                if arg == *dangerous || arg.starts_with(&format!("{dangerous}=")) {
                    return Err(GitCommandError::DangerousFlag { flag: arg.clone() });
                }
            }
        }

        // Validate working directory if provided
        if let Some(ref dir) = working_dir {
            if !dir.is_absolute() {
                return Err(GitCommandError::InvalidWorkingDirectory { path: dir.clone() });
            }
        }

        Ok(Self {
            command,
            args,
            working_dir,
        })
    }

    /// Returns the Git subcommand.
    #[must_use]
    pub fn command(&self) -> &str {
        &self.command
    }

    /// Returns the command arguments.
    #[must_use]
    pub fn args(&self) -> &[String] {
        &self.args
    }

    /// Returns the working directory, if set.
    #[must_use]
    #[allow(clippy::missing_const_for_fn)] // as_ref is not const
    pub fn working_dir(&self) -> Option<&PathBuf> {
        self.working_dir.as_ref()
    }

    /// Returns `true` if this command requires remote authentication.
    ///
    /// Commands like `clone`, `push`, `pull`, `fetch` typically need credentials.
    #[must_use]
    pub fn requires_auth(&self) -> bool {
        matches!(
            self.command.as_str(),
            "clone" | "push" | "pull" | "fetch" | "ls-remote"
        )
    }

    /// Extracts the remote URL from the command arguments, if present.
    ///
    /// This is used to find matching credentials for authentication.
    #[must_use]
    pub fn extract_remote_url(&self) -> Option<&str> {
        match self.command.as_str() {
            "clone" => {
                // clone <url> [directory]
                self.args.first().map(String::as_str)
            }
            "push" | "pull" | "fetch" => {
                // push/pull/fetch [remote] [refspec...]
                // The first non-flag argument is typically the remote
                self.args
                    .iter()
                    .find(|arg| !arg.starts_with('-'))
                    .map(String::as_str)
            }
            "ls-remote" => {
                // ls-remote [<repository>] [<refs>...]
                self.args
                    .iter()
                    .find(|arg| !arg.starts_with('-'))
                    .map(String::as_str)
            }
            _ => None,
        }
    }

    /// Builds the full command line arguments for execution.
    ///
    /// Returns a vector starting with the subcommand followed by all arguments.
    #[must_use]
    pub fn build_args(&self) -> Vec<&str> {
        let mut result = vec![self.command.as_str()];
        result.extend(self.args.iter().map(String::as_str));
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_clone_command() {
        let cmd = GitCommand::new(
            "clone",
            vec!["https://github.com/user/repo.git".to_string()],
            None,
        )
        .unwrap();

        assert_eq!(cmd.command(), "clone");
        assert_eq!(cmd.args(), &["https://github.com/user/repo.git"]);
        assert!(cmd.requires_auth());
        assert_eq!(
            cmd.extract_remote_url(),
            Some("https://github.com/user/repo.git")
        );
    }

    #[test]
    fn parse_push_command() {
        let cmd =
            GitCommand::new("push", vec!["origin".to_string(), "main".to_string()], None).unwrap();

        assert_eq!(cmd.command(), "push");
        assert!(cmd.requires_auth());
        assert_eq!(cmd.extract_remote_url(), Some("origin"));
    }

    #[test]
    fn parse_status_command() {
        let cmd = GitCommand::new("status", vec![], None).unwrap();

        assert_eq!(cmd.command(), "status");
        assert!(!cmd.requires_auth());
        assert!(cmd.extract_remote_url().is_none());
    }

    #[test]
    fn reject_empty_command() {
        let result = GitCommand::new("", vec![], None);
        assert!(matches!(result, Err(GitCommandError::EmptyCommand)));
    }

    #[test]
    fn reject_disallowed_command() {
        let result = GitCommand::new("config", vec![], None);
        assert!(matches!(
            result,
            Err(GitCommandError::CommandNotAllowed { .. })
        ));
    }

    #[test]
    fn reject_dangerous_flag() {
        let result = GitCommand::new("clone", vec!["--exec=malicious".to_string()], None);
        assert!(matches!(result, Err(GitCommandError::DangerousFlag { .. })));
    }

    #[test]
    fn reject_no_verify_flag() {
        let result = GitCommand::new("commit", vec!["--no-verify".to_string()], None);
        assert!(matches!(result, Err(GitCommandError::DangerousFlag { .. })));
    }

    #[test]
    fn reject_c_flag() {
        let result = GitCommand::new(
            "clone",
            vec!["-c".to_string(), "http.proxy=evil".to_string()],
            None,
        );
        assert!(matches!(result, Err(GitCommandError::DangerousFlag { .. })));
    }

    #[test]
    fn reject_relative_working_dir() {
        let result = GitCommand::new("status", vec![], Some(PathBuf::from("./relative/path")));
        assert!(matches!(
            result,
            Err(GitCommandError::InvalidWorkingDirectory { .. })
        ));
    }

    #[test]
    fn accept_absolute_working_dir() {
        #[cfg(windows)]
        let dir = PathBuf::from("C:\\Users\\test\\repo");
        #[cfg(not(windows))]
        let dir = PathBuf::from("/home/user/repo");

        let cmd = GitCommand::new("status", vec![], Some(dir.clone())).unwrap();
        assert_eq!(cmd.working_dir(), Some(&dir));
    }

    #[test]
    fn build_args_includes_command_and_args() {
        let cmd = GitCommand::new(
            "commit",
            vec!["-m".to_string(), "Initial commit".to_string()],
            None,
        )
        .unwrap();

        let args = cmd.build_args();
        assert_eq!(args, vec!["commit", "-m", "Initial commit"]);
    }

    #[test]
    fn all_allowed_commands_are_valid() {
        for &command in ALLOWED_COMMANDS {
            let result = GitCommand::new(command, vec![], None);
            assert!(result.is_ok(), "Command '{command}' should be allowed");
        }
    }
}
