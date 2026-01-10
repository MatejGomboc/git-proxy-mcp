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
/// Git bundles start with "# v2 bundle" or "# v3 bundle".
///
/// # Errors
///
/// Returns `BundleFailed` if the data doesn't have a valid bundle header.
pub fn validate_bundle(data: &[u8]) -> Result<(), Git2Error> {
    const V2_HEADER: &[u8] = b"# v2 bundle";
    const V3_HEADER: &[u8] = b"# v3 bundle";

    if data.starts_with(V2_HEADER) || data.starts_with(V3_HEADER) {
        debug!("valid git bundle detected");
        Ok(())
    } else {
        Err(Git2Error::BundleFailed(
            "data does not appear to be a valid git bundle".to_string(),
        ))
    }
}

/// Bundle metadata extracted from the header.
#[derive(Debug, Clone)]
pub struct BundleInfo {
    /// Bundle format version (2 or 3)
    pub version: u8,
    /// References included in the bundle
    pub refs: Vec<BundleRef>,
}

/// A reference in a git bundle.
#[derive(Debug, Clone)]
pub struct BundleRef {
    /// The object ID
    pub oid: String,
    /// The reference name (e.g., "refs/heads/main")
    pub name: String,
}

/// Parse basic info from a git bundle header.
///
/// This is a best-effort parse of the bundle header to extract
/// metadata without fully processing the bundle.
///
/// # Errors
///
/// Returns `BundleFailed` if the header is invalid UTF-8 or has an unknown version.
pub fn parse_bundle_info(data: &[u8]) -> Result<BundleInfo, Git2Error> {
    let header = std::str::from_utf8(data.get(..512).unwrap_or(data))
        .map_err(|_| Git2Error::BundleFailed("invalid bundle header encoding".to_string()))?;

    // Determine version
    let version = if header.starts_with("# v3 bundle") {
        3
    } else if header.starts_with("# v2 bundle") {
        2
    } else {
        return Err(Git2Error::BundleFailed(
            "unknown bundle version".to_string(),
        ));
    };

    // Parse references (lines after header, before empty line)
    let mut refs = Vec::new();
    let mut in_refs = false;

    for line in header.lines() {
        if line.starts_with("# v") {
            in_refs = true;
            continue;
        }

        if line.is_empty() {
            break;
        }

        if in_refs && line.len() > 40 {
            // Format: "<40-char-oid> <refname>"
            let parts: Vec<&str> = line.splitn(2, ' ').collect();
            if parts.len() == 2 {
                refs.push(BundleRef {
                    oid: parts[0].to_string(),
                    name: parts[1].to_string(),
                });
            }
        }
    }

    debug!(
        version = version,
        ref_count = refs.len(),
        "parsed bundle info"
    );

    Ok(BundleInfo { version, refs })
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
    fn validate_bundle_invalid() {
        let data = b"not a bundle";
        assert!(validate_bundle(data).is_err());
    }

    #[test]
    fn parse_bundle_info_v2() {
        let data = b"# v2 bundle\nabcd1234abcd1234abcd1234abcd1234abcd1234 refs/heads/main\n\n";
        let info = parse_bundle_info(data).unwrap();
        assert_eq!(info.version, 2);
        assert_eq!(info.refs.len(), 1);
        assert_eq!(info.refs[0].name, "refs/heads/main");
    }
}
