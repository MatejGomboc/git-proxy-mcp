//! MCP tool handlers for git-proxy-mcp.
//!
//! This module contains handlers for the various MCP tools exposed by the server.
//! Each tool has its own submodule with argument parsing, validation, and execution.
//!
//! # Available Tools
//!
//! - [`repo_clone`] — Clone a repository and stream as tar.gz (Tier 1)
//! - [`repo_push`] — Push a git bundle to a remote repository (Tier 1)
//!
//! # Security
//!
//! All tool handlers follow the security principles:
//! - Credentials never leave the user's PC
//! - No source files written to disk (bare repos only)
//! - All responses are sanitized for credential leakage

pub mod repo_clone;
pub mod repo_push;

pub use repo_clone::{handle_repo_clone, RepoCloneArgs, RepoCloneResult};
pub use repo_push::{handle_repo_push, RepoPushArgs, RepoPushResult};
