//! Authentication and credential management.
//!
//! # Security Architecture
//!
//! This module is the **only** place where credentials are handled.
//! All credential types use [`secrecy::SecretString`] to ensure:
//!
//! - Credentials are zeroised when dropped
//! - Credentials cannot accidentally be logged via `Debug`
//! - Explicit `.expose_secret()` is required to access values
//!
//! ## Rules for Contributors
//!
//! 1. **NEVER** implement `Debug` that exposes credential values
//! 2. **NEVER** implement `Display` for credential types
//! 3. **NEVER** include credentials in error messages
//! 4. **NEVER** log credentials, even at trace level
//! 5. **ALWAYS** use `SecretString` for sensitive data
//!
//! ## Credential Flow
//!
//! ```text
//! config.json → parse → SecretString → auth module → git2 callbacks
//!                           ↓
//!                    (never leaves auth module)
//!                           ↓
//!                    zeroised on drop
//! ```

pub mod credentials;
pub mod matcher;

pub use credentials::{
    AuthMethod, Credential, PatCredential, SshAgentCredential, SshKeyCredential,
};
pub use matcher::{CredentialStore, UrlPattern};
