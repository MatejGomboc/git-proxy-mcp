//! Security and safety features.
//!
//! This module provides security controls for Git operations:
//!
//! - **Audit logging**: Logs all Git operations to a file for accountability
//! - **Protected branches**: Prevents operations on protected branches
//! - **Force push blocking**: Prevents force pushes (unless explicitly allowed)
//! - **Repository allowlist/blocklist**: Controls which repositories can be accessed
//! - **Rate limiting**: Prevents runaway AI operations
//!
//! # Security Model
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                    Security Pipeline                         │
//! │                                                              │
//! │   Git Command ──┬─▶ Rate Limiter ──┐                        │
//! │                 │                   │                        │
//! │                 ├─▶ Repo Filter ───┤                        │
//! │                 │   (allow/block)  │                        │
//! │                 │                   ├──▶ Execute or Reject  │
//! │                 ├─▶ Branch Guard ──┤                        │
//! │                 │   (protected)    │                        │
//! │                 │                   │                        │
//! │                 └─▶ Push Guard ────┘                        │
//! │                     (force push)                             │
//! │                                                              │
//! │   All operations ──────────────────▶ Audit Logger           │
//! └─────────────────────────────────────────────────────────────┘
//! ```

pub mod audit;
pub mod guards;
pub mod rate_limit;

pub use audit::{AuditEvent, AuditLogger, ShutdownReason};
pub use guards::{BranchGuard, PushGuard, RepoFilter, SecurityGuard};
pub use rate_limit::RateLimiter;
