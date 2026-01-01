//! Git command proxy.
//!
//! This module handles executing Git commands as subprocesses with automatic
//! credential injection. It provides the core functionality for the MCP `git` tool.
//!
//! # Security Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                     Git Command Proxy                           │
//! │                                                                 │
//! │   1. Parse command ───────────────────────────────────────┐     │
//! │                                                           │     │
//! │   2. Validate (allowlist, blocklist) ─────────────────────┼───▶ │
//! │                                                           │     │
//! │   3. Extract remote URL ──────────────────────────────────┤     │
//! │                                                           │     │
//! │   4. Find matching credential ────────────────────────────┤     │
//! │                                                           │     │
//! │   5. Set up credential helper ────────────────────────────┤     │
//! │                                                           │     │
//! │   6. Execute git subprocess ──────────────────────────────┤     │
//! │                                                           │     │
//! │   7. Sanitise output (remove any credential leaks) ───────┤     │
//! │                                                           │     │
//! │   8. Return result ───────────────────────────────────────┘     │
//! └─────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Credential Injection
//!
//! Credentials are injected via environment variables:
//!
//! - `GIT_ASKPASS`: Points to a helper script that returns the PAT
//! - `GIT_SSH_COMMAND`: For SSH key authentication
//!
//! This approach ensures credentials are never passed on the command line
//! (which would be visible in process listings).
//!
//! # Allowed Commands
//!
//! Only a subset of Git commands are allowed to prevent misuse:
//!
//! - `clone` — Clone a repository
//! - `pull` — Fetch and merge changes
//! - `push` — Push commits to remote
//! - `fetch` — Download objects and refs
//! - `checkout` — Switch branches or restore files
//! - `status` — Show working tree status
//! - `log` — Show commit logs
//! - `diff` — Show changes
//! - `branch` — List, create, or delete branches
//! - `remote` — Manage remotes
//! - `init` — Create empty repo
//! - `add` — Stage files
//! - `commit` — Record changes
//! - `stash` — Stash changes
//! - `tag` — Create, list, delete tags
//! - `reset` — Reset HEAD
//! - `revert` — Revert commits
//! - `merge` — Join branches
//! - `rebase` — Reapply commits
//! - `show` — Show objects
//! - `ls-files` — List tracked files
//! - `ls-remote` — List remote refs
//! - `rev-parse` — Parse revision

pub mod command;
pub mod executor;
pub mod sanitiser;

pub use command::{GitCommand, GitCommandError};
pub use executor::{CommandOutput, GitExecutor};
pub use sanitiser::OutputSanitiser;
