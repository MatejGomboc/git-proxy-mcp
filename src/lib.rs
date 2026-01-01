//! git-proxy-mcp: Secure Git proxy MCP server for AI assistants
//!
//! This library provides the core functionality for a secure Git proxy
//! that allows AI assistants to work with private repositories while
//! keeping credentials on the user's machine.
//!
//! # Modules
//!
//! - [`auth`] — Authentication and credential management
//! - [`config`] — Configuration loading and validation
//! - [`error`] — Error types
//! - [`git`] — Git command parsing and execution
//! - [`mcp`] — MCP protocol implementation
//! - [`security`] — Security guards and audit logging

pub mod auth;
pub mod config;
pub mod error;
pub mod git;
pub mod mcp;
pub mod security;
