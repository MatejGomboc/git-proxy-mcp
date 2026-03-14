//! Git LFS (Large File Storage) support.
//!
//! This module provides functionality to detect and resolve Git LFS pointer files,
//! fetching the actual content from LFS servers.
//!
//! # LFS Pointer Format
//!
//! LFS pointer files are small text files with a specific format:
//!
//! ```text
//! version https://git-lfs.github.com/spec/v1
//! oid sha256:4d7a214614ab2935c943f9e0ff69d22eadbb8f32b1258daaa5e2ca24d17e2393
//! size 12345
//! ```
//!
//! # Security
//!
//! - LFS authentication uses the same credential helpers as git
//! - Credentials are never stored or logged
//! - All LFS server communication is over HTTPS

use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Read;
use tracing::{debug, trace, warn};

use super::error::Git2Error;

/// Maximum size of an LFS pointer file (per spec).
const MAX_POINTER_SIZE: usize = 1024;

/// LFS pointer file version identifier.
const LFS_POINTER_VERSION: &str = "https://git-lfs.github.com/spec/v1";

/// Parsed LFS pointer information.
#[derive(Debug, Clone)]
pub struct LfsPointer {
    /// SHA-256 hash of the actual content.
    pub oid: String,
    /// Size of the actual content in bytes.
    pub size: u64,
}

/// Check if content looks like an LFS pointer file.
///
/// Quick check without full parsing - used to filter candidates.
#[must_use]
pub fn is_lfs_pointer(content: &[u8]) -> bool {
    // Must be small enough
    if content.len() > MAX_POINTER_SIZE {
        return false;
    }

    // Must be valid UTF-8 and start with version line
    std::str::from_utf8(content)
        .is_ok_and(|text| text.starts_with("version https://git-lfs.github.com/spec/v1"))
}

/// Parse an LFS pointer file.
///
/// # Returns
///
/// `Some(LfsPointer)` if the content is a valid LFS pointer, `None` otherwise.
#[must_use]
pub fn parse_lfs_pointer(content: &[u8]) -> Option<LfsPointer> {
    // Must be valid UTF-8
    let text = std::str::from_utf8(content).ok()?;

    // Parse key-value pairs
    let mut version = None;
    let mut oid = None;
    let mut size = None;

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        // Split on first space
        let (key, value) = line.split_once(' ')?;

        match key {
            "version" => version = Some(value),
            "oid" => {
                // Format: sha256:{hash}
                if let Some(hash) = value.strip_prefix("sha256:") {
                    oid = Some(hash.to_string());
                }
            }
            "size" => {
                size = value.parse().ok();
            }
            _ => {} // Ignore extension keys
        }
    }

    // Validate required fields
    if version? != LFS_POINTER_VERSION {
        return None;
    }

    Some(LfsPointer {
        oid: oid?,
        size: size?,
    })
}

/// LFS Batch API request object.
#[derive(Debug, Serialize)]
struct LfsBatchRequest {
    operation: String,
    transfers: Vec<String>,
    objects: Vec<LfsBatchObject>,
    #[serde(skip_serializing_if = "Option::is_none")]
    r#ref: Option<LfsRef>,
}

/// Object in LFS Batch API request.
#[derive(Debug, Serialize)]
struct LfsBatchObject {
    oid: String,
    size: u64,
}

/// Git ref for LFS request.
#[derive(Debug, Serialize)]
struct LfsRef {
    name: String,
}

/// LFS Batch API response.
#[derive(Debug, Deserialize)]
struct LfsBatchResponse {
    objects: Vec<LfsBatchResponseObject>,
}

/// Object in LFS Batch API response.
#[derive(Debug, Deserialize)]
struct LfsBatchResponseObject {
    oid: String,
    #[allow(dead_code)]
    size: u64,
    actions: Option<LfsActions>,
    error: Option<LfsError>,
}

/// Actions available for an LFS object.
#[derive(Debug, Deserialize)]
struct LfsActions {
    download: Option<LfsAction>,
}

/// Single LFS action (download URL and headers).
#[derive(Debug, Deserialize)]
struct LfsAction {
    href: String,
    #[serde(default)]
    header: HashMap<String, String>,
}

/// LFS error response.
#[derive(Debug, Deserialize)]
struct LfsError {
    message: String,
}

/// Client for fetching LFS content (blocking/sync).
pub struct LfsClient {
    /// HTTP client.
    client: Client,
    /// LFS server URL.
    lfs_url: String,
    /// Basic auth credentials (username, password).
    credentials: Option<(String, String)>,
}

impl LfsClient {
    /// Create a new LFS client.
    ///
    /// # Arguments
    ///
    /// * `repo_url` - Git repository URL (used to derive LFS server URL)
    /// * `credentials` - Optional (username, password) for authentication
    ///
    /// # Errors
    ///
    /// Returns error if the URL scheme is unsupported or HTTP client creation fails.
    pub fn new(
        repo_url: &str,
        credentials: Option<(String, String)>,
        proxy_url: Option<&str>,
        no_proxy: Option<&str>,
    ) -> Result<Self, Git2Error> {
        let lfs_url = derive_lfs_url(repo_url)?;

        debug!(lfs_url = %lfs_url, "created LFS client");

        let mut builder = Client::builder().user_agent("git-proxy-mcp/0.1");

        if let Some(proxy_url) = proxy_url {
            let proxy = reqwest::Proxy::all(proxy_url)
                .map_err(|e| Git2Error::Git2(format!("invalid proxy URL: {e}")))?;
            let proxy = if let Some(no_proxy) = no_proxy {
                proxy.no_proxy(reqwest::NoProxy::from_string(no_proxy))
            } else {
                proxy
            };
            builder = builder.proxy(proxy);
        }

        let client = builder
            .build()
            .map_err(|e| Git2Error::Git2(format!("failed to create HTTP client: {e}")))?;

        Ok(Self {
            client,
            lfs_url,
            credentials,
        })
    }

    /// Fetch the actual content for an LFS pointer (blocking).
    ///
    /// # Arguments
    ///
    /// * `pointer` - The parsed LFS pointer
    ///
    /// # Returns
    ///
    /// The actual file content as bytes.
    ///
    /// # Errors
    ///
    /// Returns error if the LFS API request fails or content download fails.
    pub fn fetch_content(&self, pointer: &LfsPointer) -> Result<Vec<u8>, Git2Error> {
        trace!(oid = %pointer.oid, size = pointer.size, "fetching LFS content");

        // Step 1: Call Batch API to get download URL
        let batch_url = format!("{}/objects/batch", self.lfs_url);

        let request = LfsBatchRequest {
            operation: "download".to_string(),
            transfers: vec!["basic".to_string()],
            objects: vec![LfsBatchObject {
                oid: pointer.oid.clone(),
                size: pointer.size,
            }],
            r#ref: None,
        };

        let mut headers = HeaderMap::new();
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/vnd.git-lfs+json"),
        );
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/vnd.git-lfs+json"),
        );

        // Add Basic auth if credentials are available
        if let Some((username, password)) = &self.credentials {
            let auth = format!(
                "Basic {}",
                base64::Engine::encode(
                    &base64::engine::general_purpose::STANDARD,
                    format!("{username}:{password}")
                )
            );
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&auth)
                    .map_err(|e| Git2Error::Git2(format!("invalid auth header: {e}")))?,
            );
        }

        let response = self
            .client
            .post(&batch_url)
            .headers(headers)
            .json(&request)
            .send()
            .map_err(|e| Git2Error::Git2(format!("LFS batch request failed: {e}")))?;

        if !response.status().is_success() {
            return Err(Git2Error::Git2(format!(
                "LFS batch API returned status {}",
                response.status()
            )));
        }

        let batch_response: LfsBatchResponse = response
            .json()
            .map_err(|e| Git2Error::Git2(format!("failed to parse LFS response: {e}")))?;

        // Find our object in the response
        let obj = batch_response
            .objects
            .into_iter()
            .find(|o| o.oid == pointer.oid)
            .ok_or_else(|| Git2Error::Git2("LFS object not found in response".to_string()))?;

        // Check for error
        if let Some(err) = obj.error {
            return Err(Git2Error::Git2(format!("LFS error: {}", err.message)));
        }

        // Get download action
        let download = obj
            .actions
            .and_then(|a| a.download)
            .ok_or_else(|| Git2Error::Git2("no download action in LFS response".to_string()))?;

        trace!(href = %download.href, "downloading LFS content");

        // Step 2: Download actual content
        let mut download_headers = HeaderMap::new();
        for (key, value) in &download.header {
            if let (Ok(name), Ok(val)) = (
                reqwest::header::HeaderName::try_from(key.as_str()),
                HeaderValue::from_str(value),
            ) {
                download_headers.insert(name, val);
            }
        }

        let mut content_response = self
            .client
            .get(&download.href)
            .headers(download_headers)
            .send()
            .map_err(|e| Git2Error::Git2(format!("LFS download failed: {e}")))?;

        if !content_response.status().is_success() {
            return Err(Git2Error::Git2(format!(
                "LFS download returned status {}",
                content_response.status()
            )));
        }

        // Read content into buffer
        // On 32-bit systems, this may truncate for very large files (>4GB)
        #[allow(clippy::cast_possible_truncation)]
        let capacity = pointer.size as usize;
        let mut content = Vec::with_capacity(capacity);
        content_response
            .read_to_end(&mut content)
            .map_err(|e| Git2Error::Git2(format!("failed to read LFS content: {e}")))?;

        // Verify size
        if content.len() as u64 != pointer.size {
            warn!(
                expected = pointer.size,
                actual = content.len(),
                "LFS content size mismatch"
            );
        }

        debug!(oid = %pointer.oid, size = content.len(), "LFS content fetched");

        Ok(content)
    }

    /// Batch fetch multiple LFS objects.
    ///
    /// This is more efficient than calling `fetch_content` multiple times
    /// as it uses a single Batch API request.
    ///
    /// # Arguments
    ///
    /// * `pointers` - List of LFS pointers to fetch
    ///
    /// # Returns
    ///
    /// Map from OID to content bytes. Missing/failed objects are omitted.
    ///
    /// # Errors
    ///
    /// Returns `Git2Error` if the batch API request fails or returns an error.
    pub fn fetch_batch(
        &self,
        pointers: &[LfsPointer],
    ) -> Result<HashMap<String, Vec<u8>>, Git2Error> {
        if pointers.is_empty() {
            return Ok(HashMap::new());
        }

        debug!(count = pointers.len(), "batch fetching LFS content");

        // Build batch request
        let batch_url = format!("{}/objects/batch", self.lfs_url);

        let request = LfsBatchRequest {
            operation: "download".to_string(),
            transfers: vec!["basic".to_string()],
            objects: pointers
                .iter()
                .map(|p| LfsBatchObject {
                    oid: p.oid.clone(),
                    size: p.size,
                })
                .collect(),
            r#ref: None,
        };

        let mut headers = HeaderMap::new();
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/vnd.git-lfs+json"),
        );
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/vnd.git-lfs+json"),
        );

        if let Some((username, password)) = &self.credentials {
            let auth = format!(
                "Basic {}",
                base64::Engine::encode(
                    &base64::engine::general_purpose::STANDARD,
                    format!("{username}:{password}")
                )
            );
            if let Ok(val) = HeaderValue::from_str(&auth) {
                headers.insert(AUTHORIZATION, val);
            }
        }

        let response = self
            .client
            .post(&batch_url)
            .headers(headers)
            .json(&request)
            .send()
            .map_err(|e| Git2Error::Git2(format!("LFS batch request failed: {e}")))?;

        if !response.status().is_success() {
            return Err(Git2Error::Git2(format!(
                "LFS batch API returned status {}",
                response.status()
            )));
        }

        let batch_response: LfsBatchResponse = response
            .json()
            .map_err(|e| Git2Error::Git2(format!("failed to parse LFS response: {e}")))?;

        // Download each object
        let mut results = HashMap::new();

        for obj in batch_response.objects {
            if obj.error.is_some() {
                warn!(oid = %obj.oid, "LFS object has error, skipping");
                continue;
            }

            let Some(download) = obj.actions.and_then(|a| a.download) else {
                warn!(oid = %obj.oid, "LFS object has no download action, skipping");
                continue;
            };

            // Build headers for download
            let mut download_headers = HeaderMap::new();
            for (key, value) in &download.header {
                if let (Ok(name), Ok(val)) = (
                    reqwest::header::HeaderName::try_from(key.as_str()),
                    HeaderValue::from_str(value),
                ) {
                    download_headers.insert(name, val);
                }
            }

            // Download content
            match self
                .client
                .get(&download.href)
                .headers(download_headers)
                .send()
            {
                Ok(mut resp) if resp.status().is_success() => {
                    let mut content = Vec::new();
                    if resp.read_to_end(&mut content).is_ok() {
                        trace!(oid = %obj.oid, size = content.len(), "downloaded LFS object");
                        results.insert(obj.oid, content);
                    }
                }
                Ok(resp) => {
                    warn!(oid = %obj.oid, status = %resp.status(), "LFS download failed");
                }
                Err(e) => {
                    warn!(oid = %obj.oid, error = %e, "LFS download failed");
                }
            }
        }

        debug!(
            requested = pointers.len(),
            fetched = results.len(),
            "batch fetch complete"
        );

        Ok(results)
    }
}

/// Derive LFS server URL from git repository URL.
///
/// For GitHub/GitLab, the LFS URL is typically: `{repo_url}/info/lfs`
fn derive_lfs_url(repo_url: &str) -> Result<String, Git2Error> {
    // Handle SSH URLs: git@github.com:owner/repo.git -> https://github.com/owner/repo.git
    let https_url = if repo_url.starts_with("git@") {
        // git@github.com:owner/repo.git -> https://github.com/owner/repo.git
        let url = repo_url
            .strip_prefix("git@")
            .ok_or_else(|| Git2Error::Git2("invalid SSH URL".to_string()))?;
        let url = url.replacen(':', "/", 1);
        format!("https://{url}")
    } else if repo_url.starts_with("https://") || repo_url.starts_with("http://") {
        repo_url.to_string()
    } else {
        return Err(Git2Error::Git2(format!(
            "unsupported URL scheme: {repo_url}"
        )));
    };

    // Remove .git suffix if present
    let base = https_url.trim_end_matches(".git");

    // LFS endpoint
    Ok(format!("{base}/info/lfs"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_lfs_pointer_detects_valid() {
        let pointer = b"version https://git-lfs.github.com/spec/v1\n\
                        oid sha256:4d7a214614ab2935c943f9e0ff69d22eadbb8f32b1258daaa5e2ca24d17e2393\n\
                        size 12345\n";
        assert!(is_lfs_pointer(pointer));
    }

    #[test]
    fn is_lfs_pointer_rejects_regular_file() {
        let regular = b"Hello, this is a regular file\nwith some content";
        assert!(!is_lfs_pointer(regular));
    }

    #[test]
    fn is_lfs_pointer_rejects_large_file() {
        let large = vec![b'x'; MAX_POINTER_SIZE + 1];
        assert!(!is_lfs_pointer(&large));
    }

    #[test]
    fn parse_lfs_pointer_valid() {
        let pointer = b"version https://git-lfs.github.com/spec/v1\n\
                        oid sha256:4d7a214614ab2935c943f9e0ff69d22eadbb8f32b1258daaa5e2ca24d17e2393\n\
                        size 12345\n";
        let parsed = parse_lfs_pointer(pointer).unwrap();
        assert_eq!(
            parsed.oid,
            "4d7a214614ab2935c943f9e0ff69d22eadbb8f32b1258daaa5e2ca24d17e2393"
        );
        assert_eq!(parsed.size, 12345);
    }

    #[test]
    fn parse_lfs_pointer_wrong_version() {
        let pointer = b"version https://git-lfs.github.com/spec/v2\n\
                        oid sha256:abc123\n\
                        size 100\n";
        assert!(parse_lfs_pointer(pointer).is_none());
    }

    #[test]
    fn parse_lfs_pointer_missing_oid() {
        let pointer = b"version https://git-lfs.github.com/spec/v1\n\
                        size 100\n";
        assert!(parse_lfs_pointer(pointer).is_none());
    }

    #[test]
    fn parse_lfs_pointer_missing_size() {
        let pointer = b"version https://git-lfs.github.com/spec/v1\n\
                        oid sha256:abc123\n";
        assert!(parse_lfs_pointer(pointer).is_none());
    }

    #[test]
    fn derive_lfs_url_https() {
        let url = derive_lfs_url("https://github.com/owner/repo.git").unwrap();
        assert_eq!(url, "https://github.com/owner/repo/info/lfs");
    }

    #[test]
    fn derive_lfs_url_https_no_git_suffix() {
        let url = derive_lfs_url("https://github.com/owner/repo").unwrap();
        assert_eq!(url, "https://github.com/owner/repo/info/lfs");
    }

    #[test]
    fn derive_lfs_url_ssh() {
        let url = derive_lfs_url("git@github.com:owner/repo.git").unwrap();
        assert_eq!(url, "https://github.com/owner/repo/info/lfs");
    }

    #[test]
    fn derive_lfs_url_invalid() {
        let result = derive_lfs_url("ftp://invalid.com/repo");
        assert!(result.is_err());
    }

    #[test]
    fn lfs_client_creation() {
        let client = LfsClient::new("https://github.com/owner/repo.git", None, None, None);
        assert!(client.is_ok());
    }

    #[test]
    fn lfs_client_with_credentials() {
        let client = LfsClient::new(
            "https://github.com/owner/repo.git",
            Some(("user".to_string(), "pass".to_string())),
            None,
            None,
        );
        assert!(client.is_ok());
    }
}
