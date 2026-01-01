//! git-proxy-mcp: Secure Git proxy MCP server for AI assistants
//!
//! This library provides the core functionality for a secure Git proxy
//! that allows AI assistants to work with private repositories using
//! the user's existing Git credential configuration.
//!
//! # Architecture
//!
//! The MCP server acts as a simple proxy that spawns git commands on behalf
//! of AI assistants. It does NOT store credentials — instead, it relies on
//! the user's existing git configuration:
//!
//! - Credential helpers (macOS Keychain, Windows Credential Manager, libsecret)
//! - SSH agent for SSH key authentication
//! - Standard ~/.gitconfig settings
//!
//! # Modules
//!
//! - [`config`] — Configuration loading and validation
//! - [`error`] — Error types
//! - [`git`] — Git command parsing and execution
//! - [`mcp`] — MCP protocol implementation
//! - [`security`] — Security guards and audit logging

pub mod config;
pub mod error;
pub mod git;
pub mod mcp;
pub mod security;
