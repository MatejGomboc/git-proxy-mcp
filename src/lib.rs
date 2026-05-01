//! git-proxy-mcp: Secure Git proxy MCP server for AI assistants
//!
//! This library provides the core functionality for a secure Git proxy
//! that allows AI assistants to work with private repositories using
//! the user's existing Git credential configuration.
//!
//! # Architecture
//!
//! The MCP server acts as a credential relay between Git providers and AI VMs:
//!
//! ```text
//! GitHub → User's PC (authenticate) → AI's VM (files)
//! ```
//!
//! Credentials NEVER leave the user's PC. Only file contents flow to the AI.
//!
//! ## Tier 1: Single-Response Streaming
//!
//! - Clone: Fetch to bare repo, stream tree as tar.gz (in memory)
//! - Push: Receive bundle, authenticated push
//! - Memory usage: O(repo size)
//! - Tools: `repo_clone`, `repo_push`
//!
//! ## Tier 2: Chunked Streaming
//!
//! - Stream in chunks for large repos
//! - Memory usage: O(chunk size)
//! - Resume interrupted transfers
//! - Progress reporting
//! - Tools: `repo_clone_start`, `repo_clone_chunk`, `repo_clone_status`, `repo_clone_cancel`
//!
//! ## Other tools
//!
//! Independent of the Tier 1 / Tier 2 split:
//!
//! - `repo_pull` — Incremental sync since a known commit
//! - `repo_diff` — Diff between two commits
//! - `repo_refs` — List remote branches and tags
//! - `helper_script` — Get a Python helper for parsing tool responses
//!
//! # Credential Handling
//!
//! Uses git2's callback system (no credentials stored):
//!
//! - SSH keys: Via ssh-agent (private key never leaves agent)
//! - HTTPS tokens: Via system credential helpers
//!
//! # Modules
//!
//! - [`config`] — Configuration loading and validation
//! - [`error`] — Error types
//! - [`git2_ops`] — git2 library operations (credential relay)
//! - [`mcp`] — MCP protocol implementation
//! - [`security`] — Security guards and audit logging
//! - [`session`] — Session management for tracking cloned repositories
//! - [`streaming`] — In-memory tar/bundle streaming

pub mod config;
pub mod error;
pub mod git2_ops;
pub mod mcp;
pub mod security;
pub mod session;
pub mod streaming;
