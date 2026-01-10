//! git2-based Git operations for the credential relay.
//!
//! This module provides in-process git operations using the git2 library,
//! replacing the subprocess-based approach. Key benefits:
//!
//! - Credential callbacks (SSH agent, system credential helpers)
//! - Bare repository operations (no working tree, no source files on disk)
//! - Direct access to git object database for streaming
//!
//! # Security Model
//!
//! Credentials are handled via git2's callback system:
//! - SSH keys: Accessed via ssh-agent (private key never leaves agent)
//! - HTTPS tokens: Retrieved from system credential helpers (never stored)
//!
//! # Modules
//!
//! - [`auth`] — Credential callback setup
//! - [`clone`] — Bare repository fetch operations
//! - [`diff`] — Diff generation between commits
//! - [`push`] — Bundle processing and authenticated push
//! - [`refs`] — Remote reference listing (branches/tags)
//! - [`error`] — Error types for git2 operations

pub mod auth;
pub mod clone;
pub mod diff;
pub mod error;
pub mod push;
pub mod refs;
