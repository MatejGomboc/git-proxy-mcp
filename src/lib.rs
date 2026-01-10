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
//! - Tools: `repo/clone`, `repo/push`
//!
//! ## Tier 2: Chunked Streaming
//!
//! - Stream in chunks for large repos
//! - Memory usage: O(chunk size)
//! - Resume interrupted transfers
//! - Progress reporting
//! - Tools: `repo/clone_start`, `repo/clone_chunk`, `repo/clone_cancel`
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
