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
//! - [`repo_push`] — Push a git bundle to a remote repository
//!
//! ## Tier 2 (Chunked Streaming)
//!
//! - [`repo_clone_start`] — Start a chunked clone, returns session info
//! - [`repo_clone_chunk`] — Get a chunk from a streaming session
//!
//! # Security
//!
//! All tool handlers follow the security principles:
//! - Credentials never leave the user's PC
//! - No source files written to disk (bare repos only)
//! - All responses are sanitized for credential leakage

pub mod repo_clone;
pub mod repo_clone_chunk;
pub mod repo_clone_start;
pub mod repo_push;

pub use repo_clone::{handle_repo_clone, RepoCloneArgs, RepoCloneResult};
pub use repo_clone_chunk::{
    handle_repo_clone_cancel, handle_repo_clone_chunk, RepoCloneCancelArgs, RepoCloneCancelResult,
    RepoCloneChunkArgs, RepoCloneChunkResult,
};
pub use repo_clone_start::{handle_repo_clone_start, RepoCloneStartArgs, RepoCloneStartResult};
pub use repo_push::{handle_repo_push, RepoPushArgs, RepoPushResult};
