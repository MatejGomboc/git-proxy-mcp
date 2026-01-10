//! In-memory streaming for repository data transfer.
//!
//! This module handles converting git data into transfer formats
//! without writing source files to disk:
//!
//! - [`tar`] — Create tar.gz archives from git trees (in memory)
//! - [`bundle`] — Handle git bundle format for push operations
//!
//! # Design Principle
//!
//! All operations work on git objects (blobs, trees) directly,
//! streaming their contents into archives without creating a working tree.
//!
//! # Tier 1 vs Tier 2
//!
//! - **Tier 1** (current): Buffer entire archive in `Vec<u8>`
//! - **Tier 2** (future): Stream chunks progressively

pub mod bundle;
pub mod tar;
