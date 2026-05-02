//! Git bundle handling for push operations.
//!
//! Git bundles are self-contained archives that include git objects
//! and references. They can be used to transfer commits without
//! network access to the original remote.
//!
//! # How Push Works
//!
//! 1. AI creates a bundle of commits to push: `git bundle create`
//! 2. AI sends bundle to MCP server (base64 encoded)
//! 3. MCP server decodes and unbundles into temp repo
//! 4. MCP server pushes to remote with credentials
//! 5. Temp repo is cleaned up

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use tracing::debug;

use crate::git2_ops::error::Git2Error;

/// Decode a base64-encoded git bundle.
///
/// # Arguments
///
/// - `encoded`: Base64-encoded bundle data from the AI
///
/// # Returns
///
/// The raw bundle bytes, ready for unbundling.
///
/// # Errors
///
/// Returns `BundleFailed` if the base64 decoding fails.
pub fn decode_bundle(encoded: &str) -> Result<Vec<u8>, Git2Error> {
    debug!(encoded_len = encoded.len(), "decoding bundle");

    STANDARD
        .decode(encoded)
        .map_err(|e| Git2Error::BundleFailed(format!("invalid base64: {e}")))
}

/// Validate that data looks like a git bundle.
///
/// Git bundles start with `# v2 bundle` / `# v3 bundle` (git < 2.53)
/// or `# v2 git bundle` / `# v3 git bundle` (git >= 2.53).
///
/// # Errors
///
/// Returns `BundleFailed` if the data doesn't have a valid bundle header.
pub fn validate_bundle(data: &[u8]) -> Result<(), Git2Error> {
    const V2_HEADER: &[u8] = b"# v2 bundle";
    const V2_GIT_HEADER: &[u8] = b"# v2 git bundle";
    const V3_HEADER: &[u8] = b"# v3 bundle";
    const V3_GIT_HEADER: &[u8] = b"# v3 git bundle";

    if data.starts_with(V2_HEADER)
        || data.starts_with(V2_GIT_HEADER)
        || data.starts_with(V3_HEADER)
        || data.starts_with(V3_GIT_HEADER)
    {
        debug!("valid git bundle detected");
        Ok(())
    } else {
        Err(Git2Error::BundleFailed(
            "data does not appear to be a valid git bundle".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_bundle_valid_base64() {
        let original = b"# v2 bundle\n";
        let encoded = STANDARD.encode(original);
        let decoded = decode_bundle(&encoded).unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn decode_bundle_invalid_base64() {
        let result = decode_bundle("not valid base64!!!");
        assert!(result.is_err());
    }

    #[test]
    fn validate_bundle_v2() {
        let data = b"# v2 bundle\nsome content";
        assert!(validate_bundle(data).is_ok());
    }

    #[test]
    fn validate_bundle_v3() {
        let data = b"# v3 bundle\nsome content";
        assert!(validate_bundle(data).is_ok());
    }

    #[test]
    fn validate_bundle_v2_git() {
        let data = b"# v2 git bundle\nsome content";
        assert!(validate_bundle(data).is_ok());
    }

    #[test]
    fn validate_bundle_v3_git() {
        let data = b"# v3 git bundle\nsome content";
        assert!(validate_bundle(data).is_ok());
    }

    #[test]
    fn validate_bundle_invalid() {
        let data = b"not a bundle";
        assert!(validate_bundle(data).is_err());
    }
}
