//! Handler for the `repo/push` MCP tool.
//!
//! This tool receives a git bundle from the AI and pushes it to a remote
//! repository using the user's credentials.
//!
//! # Data Flow
//!
//! ```text
//! 1. AI creates git bundle: git bundle create changes.bundle HEAD~N..HEAD
//! 2. AI sends base64-encoded bundle to MCP
//! 3. MCP decodes and unbundles into temp bare repo
//! 4. MCP pushes to remote with credential callbacks
//! 5. Temp dir auto-cleaned
//! ```
//!
//! # Security
//!
//! - Uses credential callbacks (SSH agent, credential helpers)
//! - Only bundle file touches disk (not source files)
//! - Protected branch guards enforced

use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use crate::git2_ops::auth::sanitize_url_for_logging;
use crate::git2_ops::error::Git2Error;
use crate::git2_ops::push::{push_bundle, PushOptions2};
use crate::streaming::bundle::{decode_bundle, validate_bundle};

/// Arguments for the `repo/push` tool.
#[derive(Debug, Clone, Deserialize)]
pub struct RepoPushArgs {
    /// Base64-encoded git bundle
    pub bundle: String,

    /// Target repository URL (https:// or git@)
    pub url: String,

    /// Target branch to push to
    pub branch: String,

    /// Force push (use with caution!)
    #[serde(default)]
    pub force: bool,
}

/// Result of a successful `repo/push` operation.
#[derive(Debug, Clone, Serialize)]
pub struct RepoPushResult {
    /// The branch that was pushed to
    pub branch: String,

    /// The commit SHA that was pushed
    pub commit: String,

    /// Whether force push was used
    pub force: bool,

    /// Remote URL (sanitized)
    pub remote_url: String,
}

/// Error from repo/push operation (safe for display).
#[derive(Debug)]
pub struct RepoPushError {
    /// Error message (credential-safe)
    pub message: String,
}

impl std::fmt::Display for RepoPushError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl From<Git2Error> for RepoPushError {
    fn from(err: Git2Error) -> Self {
        Self {
            message: err.to_string(),
        }
    }
}

/// Handle the `repo/push` tool call.
///
/// This function:
/// 1. Decodes the base64 bundle
/// 2. Validates it's a valid git bundle
/// 3. Unbundles into a temp bare repo
/// 4. Pushes to the remote with credentials
///
/// # Arguments
///
/// - `args`: The tool arguments from the MCP request
///
/// # Returns
///
/// A `RepoPushResult` with the push details, or an error.
///
/// # Errors
///
/// Returns `RepoPushError` if:
/// - Bundle decoding fails
/// - Bundle validation fails
/// - Push operation fails (auth, network, etc.)
///
/// # Security
///
/// - Credentials are handled via git2 callbacks (never stored)
/// - Only the bundle file touches disk
/// - Protected branch guards should be checked by caller
pub fn handle_repo_push(args: RepoPushArgs) -> Result<RepoPushResult, RepoPushError> {
    info!(
        url = %sanitize_url_for_logging(&args.url),
        branch = %args.branch,
        force = args.force,
        bundle_len = args.bundle.len(),
        "repo/push tool called"
    );

    // Decode the bundle
    let bundle_data = decode_bundle(&args.bundle)?;

    debug!(bundle_size = bundle_data.len(), "bundle decoded");

    // Validate it's a git bundle
    validate_bundle(&bundle_data)?;

    // Push the bundle
    let push_opts = PushOptions2 {
        branch: args.branch.clone(),
        force: args.force,
    };

    let result = push_bundle(&bundle_data, &args.url, push_opts)?;

    info!(
        commit = %result.commit,
        branch = %result.branch,
        "repo/push complete"
    );

    Ok(RepoPushResult {
        branch: result.branch,
        commit: result.commit,
        force: args.force,
        remote_url: sanitize_url_for_logging(&args.url),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_push_args_defaults() {
        let json = r#"{
            "bundle": "SGVsbG8=",
            "url": "https://github.com/owner/repo.git",
            "branch": "main"
        }"#;
        let args: RepoPushArgs = serde_json::from_str(json).unwrap();
        assert_eq!(args.url, "https://github.com/owner/repo.git");
        assert_eq!(args.branch, "main");
        assert!(!args.force);
    }

    #[test]
    fn repo_push_args_with_force() {
        let json = r#"{
            "bundle": "SGVsbG8=",
            "url": "https://github.com/owner/repo.git",
            "branch": "feature",
            "force": true
        }"#;
        let args: RepoPushArgs = serde_json::from_str(json).unwrap();
        assert!(args.force);
    }

    #[test]
    fn repo_push_result_serializes() {
        let result = RepoPushResult {
            branch: "main".to_string(),
            commit: "abc123".to_string(),
            force: false,
            remote_url: "https://github.com/owner/repo.git".to_string(),
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"branch\":\"main\""));
        assert!(json.contains("\"force\":false"));
    }

    // Integration tests that require network access are in tests/
}
