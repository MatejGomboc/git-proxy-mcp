//! MCP tool handlers for git-proxy-mcp.
//!
//! This module contains handlers for the various MCP tools exposed by the server.
//! Each tool has its own submodule with argument parsing, validation, and execution.
//!
//! # Available Tools
//!
//! ## Tier 1 (Single Response)
//!
//! - [`repo_clone`] — Clone a repository and stream as tar.gz
//! - [`repo_diff`] — Get diff between two commits
//! - [`repo_pull`] — Incremental sync (delta since known commit)
//! - [`repo_push`] — Push a git bundle to a remote repository
//! - [`repo_refs`] — List branches and tags without cloning
//!
//! ## Tier 2 (Chunked Streaming)
//!
//! - [`repo_clone_start`] — Start a chunked clone, returns session info
//! - [`repo_clone_chunk`] — Get a chunk from a streaming session
//!
//! ## Utilities
//!
//! - [`helper_script`] — Get a Python helper script for easier result handling
//!
//! # Security
//!
//! All tool handlers follow the security principles:
//! - Credentials never leave the user's PC
//! - No source files written to disk (bare repos only)
//! - All responses are sanitized for credential leakage

pub mod helper_script;
pub mod repo_clone;
pub mod repo_clone_chunk;
pub mod repo_clone_start;
pub mod repo_diff;
pub mod repo_pull;
pub mod repo_push;
pub mod repo_refs;

pub use helper_script::{handle_helper_script, HelperScriptResult};
pub use repo_clone::{handle_repo_clone, RepoCloneArgs, RepoCloneResult};
pub use repo_clone_chunk::{
    handle_repo_clone_cancel, handle_repo_clone_chunk, handle_repo_clone_status,
    RepoCloneCancelArgs, RepoCloneCancelResult, RepoCloneChunkArgs, RepoCloneChunkResult,
    RepoCloneStatusArgs, RepoCloneStatusResult,
};
pub use repo_clone_start::{handle_repo_clone_start, RepoCloneStartArgs, RepoCloneStartResult};
pub use repo_diff::{handle_repo_diff, RepoDiffArgs, RepoDiffResult};
pub use repo_pull::{handle_repo_pull, RepoPullArgs, RepoPullResult};
pub use repo_push::{handle_repo_push, RepoPushArgs, RepoPushResult};
pub use repo_refs::{handle_repo_refs, RepoRefsArgs, RepoRefsResult};
