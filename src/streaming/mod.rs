//! In-memory streaming for repository data transfer.
//!
//! This module handles converting git data into transfer formats
//! without writing source files to disk:
//!
//! - [`tar`] — Create tar.gz archives from git trees (in memory)
//! - [`bundle`] — Handle git bundle format for push operations
//! - [`chunked`] — Chunked streaming for Tier 2 large repo support
//!
//! # Design Principle
//!
//! All operations work on git objects (blobs, trees) directly,
//! streaming their contents into archives without creating a working tree.
//!
//! # Tier 1 vs Tier 2
//!
//! - **Tier 1**: Buffer entire archive in `Vec<u8>`, single response
//! - **Tier 2**: Chunked streaming, multiple requests for large repos

pub mod bundle;
pub mod chunked;
pub mod tar;
