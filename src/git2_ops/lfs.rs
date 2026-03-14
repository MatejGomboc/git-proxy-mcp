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
use std::time::Duration;
use tracing::{debug, trace, warn};

use super::error::Git2Error;
use crate::config::LfsConfig;
use crate::mcp::ProgressSender;

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

/// Size of chunks used for LFS content download with progress reporting.
const LFS_DOWNLOAD_CHUNK_SIZE: usize = 64 * 1024; // 64 KB

/// Result of a batch fetch operation.
#[derive(Debug)]
pub struct LfsBatchResult {
    /// Map from OID to content bytes.
    pub contents: HashMap<String, Vec<u8>>,
    /// Number of objects skipped because they exceeded size limits.
    pub skipped_too_large: usize,
}

/// Client for fetching LFS content (blocking/sync).
pub struct LfsClient {
    /// HTTP client.
    client: Client,
    /// LFS server URL.
    lfs_url: String,
    /// Basic auth credentials (username, password).
    credentials: Option<(String, String)>,
    /// Optional progress sender for real-time updates.
    progress: Option<ProgressSender>,
    /// Maximum number of retry attempts for transient errors.
    retry_max_attempts: u32,
    /// Initial backoff delay in milliseconds before the first retry.
    retry_initial_backoff_ms: u64,
    /// Maximum backoff delay in milliseconds between retries.
    retry_max_backoff_ms: u64,
    /// Multiplier applied to backoff delay after each retry.
    retry_backoff_multiplier: f64,
    /// Maximum size in bytes for a single LFS object (None = unlimited).
    max_object_size: Option<u64>,
    /// Maximum total size in bytes for all LFS objects (None = unlimited).
    max_total_size: Option<u64>,
}

impl LfsClient {
    /// Create a new LFS client.
    ///
    /// # Arguments
    ///
    /// * `repo_url` - Git repository URL (used to derive LFS server URL)
    /// * `credentials` - Optional (username, password) for authentication
    /// * `proxy_url` - Optional proxy URL for HTTP requests
    /// * `no_proxy` - Optional comma-separated list of hosts to bypass proxy
    /// * `lfs_config` - LFS configuration (retry behaviour, size limits)
    /// * `progress` - Optional progress sender for real-time updates
    ///
    /// # Errors
    ///
    /// Returns error if the URL scheme is unsupported or HTTP client creation fails.
    pub fn new(
        repo_url: &str,
        credentials: Option<(String, String)>,
        proxy_url: Option<&str>,
        no_proxy: Option<&str>,
        lfs_config: &LfsConfig,
        progress: Option<ProgressSender>,
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
            progress,
            retry_max_attempts: lfs_config.retry_max_attempts,
            retry_initial_backoff_ms: lfs_config.retry_initial_backoff_ms,
            retry_max_backoff_ms: lfs_config.retry_max_backoff_ms,
            retry_backoff_multiplier: lfs_config.retry_backoff_multiplier,
            max_object_size: lfs_config.max_object_size,
            max_total_size: lfs_config.max_total_size,
        })
    }

    /// Determines whether an HTTP status code is transient and should be retried.
    #[allow(clippy::missing_const_for_fn)] // matches! macro prevents const
    fn is_transient_status(status: reqwest::StatusCode) -> bool {
        matches!(status.as_u16(), 429 | 500 | 502 | 503 | 504)
    }

    /// Determines whether a reqwest error is transient (connection/timeout).
    fn is_transient_error(err: &reqwest::Error) -> bool {
        err.is_connect() || err.is_timeout()
    }

    /// Calculates the next backoff delay using exponential backoff.
    ///
    /// Returns the new delay in milliseconds, capped at `max_backoff_ms`.
    /// Precision loss from `u64 -> f64` is acceptable for backoff timing.
    #[allow(clippy::cast_precision_loss)]
    #[allow(clippy::cast_possible_truncation)]
    #[allow(clippy::cast_sign_loss)]
    fn next_backoff(&self, current_ms: u64) -> u64 {
        ((current_ms as f64 * self.retry_backoff_multiplier) as u64).min(self.retry_max_backoff_ms)
    }

    /// Downloads content from a URL with retry and exponential backoff.
    ///
    /// Only transient errors (HTTP 429, 500, 502, 503, 504, and
    /// connection/timeout errors) are retried. Client errors such as
    /// 401, 403, 404 are not retried.
    fn download_with_retry(&self, url: &str, headers: &HeaderMap) -> Result<Vec<u8>, Git2Error> {
        let mut attempt = 0u32;
        let mut delay_ms = self.retry_initial_backoff_ms;

        loop {
            let result = self.client.get(url).headers(headers.clone()).send();

            match result {
                Ok(mut response) => {
                    if response.status().is_success() {
                        let mut content = Vec::new();
                        response.read_to_end(&mut content).map_err(|e| {
                            Git2Error::Git2(format!("failed to read LFS content: {e}"))
                        })?;
                        return Ok(content);
                    }

                    let status = response.status();
                    if Self::is_transient_status(status) && attempt + 1 < self.retry_max_attempts {
                        attempt += 1;
                        warn!(
                            attempt = attempt,
                            max_attempts = self.retry_max_attempts,
                            status = %status,
                            url = %url,
                            delay_ms = delay_ms,
                            "LFS download returned transient error, retrying"
                        );
                        std::thread::sleep(Duration::from_millis(delay_ms));
                        delay_ms = self.next_backoff(delay_ms);
                        continue;
                    }

                    return Err(Git2Error::Git2(format!(
                        "LFS download returned status {status}"
                    )));
                }
                Err(e) => {
                    if Self::is_transient_error(&e) && attempt + 1 < self.retry_max_attempts {
                        attempt += 1;
                        warn!(
                            attempt = attempt,
                            max_attempts = self.retry_max_attempts,
                            error = %e,
                            url = %url,
                            delay_ms = delay_ms,
                            "LFS download failed with transient error, retrying"
                        );
                        std::thread::sleep(Duration::from_millis(delay_ms));
                        delay_ms = self.next_backoff(delay_ms);
                        continue;
                    }

                    return Err(Git2Error::Git2(format!("LFS download failed: {e}")));
                }
            }
        }
    }

    /// Sends a POST request with retry and exponential backoff.
    ///
    /// Used for the Batch API POST request.
    fn post_with_retry(
        &self,
        url: &str,
        headers: &HeaderMap,
        body: &impl Serialize,
    ) -> Result<reqwest::blocking::Response, Git2Error> {
        let mut attempt = 0u32;
        let mut delay_ms = self.retry_initial_backoff_ms;

        loop {
            let result = self
                .client
                .post(url)
                .headers(headers.clone())
                .json(body)
                .send();

            match result {
                Ok(response) => {
                    if response.status().is_success() {
                        return Ok(response);
                    }

                    let status = response.status();
                    if Self::is_transient_status(status) && attempt + 1 < self.retry_max_attempts {
                        attempt += 1;
                        warn!(
                            attempt = attempt,
                            max_attempts = self.retry_max_attempts,
                            status = %status,
                            url = %url,
                            delay_ms = delay_ms,
                            "LFS batch POST returned transient error, retrying"
                        );
                        std::thread::sleep(Duration::from_millis(delay_ms));
                        delay_ms = self.next_backoff(delay_ms);
                        continue;
                    }

                    return Err(Git2Error::Git2(format!(
                        "LFS batch API returned status {status}"
                    )));
                }
                Err(e) => {
                    if Self::is_transient_error(&e) && attempt + 1 < self.retry_max_attempts {
                        attempt += 1;
                        warn!(
                            attempt = attempt,
                            max_attempts = self.retry_max_attempts,
                            error = %e,
                            url = %url,
                            delay_ms = delay_ms,
                            "LFS batch POST failed with transient error, retrying"
                        );
                        std::thread::sleep(Duration::from_millis(delay_ms));
                        delay_ms = self.next_backoff(delay_ms);
                        continue;
                    }

                    return Err(Git2Error::Git2(format!("LFS batch request failed: {e}")));
                }
            }
        }
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
    /// Returns error if the LFS API request fails, content download fails,
    /// or the object exceeds `max_object_size`.
    pub fn fetch_content(&self, pointer: &LfsPointer) -> Result<Vec<u8>, Git2Error> {
        trace!(oid = %pointer.oid, size = pointer.size, "fetching LFS content");

        // Check per-object size limit before downloading
        if let Some(max_size) = self.max_object_size {
            if pointer.size > max_size {
                return Err(Git2Error::Git2(format!(
                    "LFS object {} exceeds max_object_size ({} > {})",
                    pointer.oid, pointer.size, max_size
                )));
            }
        }

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

        let response = self.post_with_retry(&batch_url, &headers, &request)?;

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

        // Step 2: Download actual content with retry
        let mut download_headers = HeaderMap::new();
        for (key, value) in &download.header {
            if let (Ok(name), Ok(val)) = (
                reqwest::header::HeaderName::try_from(key.as_str()),
                HeaderValue::from_str(value),
            ) {
                download_headers.insert(name, val);
            }
        }

        // Use chunked reading with progress if a progress sender is available
        let content = if self.progress.is_some() {
            self.download_with_progress(&download.href, &download_headers, pointer)?
        } else {
            self.download_with_retry(&download.href, &download_headers)?
        };

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

    /// Downloads content with chunked reading and progress reporting.
    ///
    /// Reads in 64KB chunks, sending progress updates via the `ProgressSender`.
    /// Falls back to retry logic for the initial request.
    fn download_with_progress(
        &self,
        url: &str,
        headers: &HeaderMap,
        pointer: &LfsPointer,
    ) -> Result<Vec<u8>, Git2Error> {
        // We use retry logic for the initial request establishment
        let mut attempt = 0u32;
        let mut delay_ms = self.retry_initial_backoff_ms;

        loop {
            let result = self.client.get(url).headers(headers.clone()).send();

            match result {
                Ok(mut response) => {
                    if response.status().is_success() {
                        // Read in chunks with progress
                        // On 32-bit systems, this may truncate for very large files (>4GB)
                        #[allow(clippy::cast_possible_truncation)]
                        let capacity = pointer.size as usize;
                        let mut content = Vec::with_capacity(capacity);
                        let mut buf = vec![0u8; LFS_DOWNLOAD_CHUNK_SIZE];
                        let mut bytes_read = 0usize;

                        loop {
                            let n = response.read(&mut buf).map_err(|e| {
                                Git2Error::Git2(format!("failed to read LFS content: {e}"))
                            })?;
                            if n == 0 {
                                break;
                            }
                            content.extend_from_slice(&buf[..n]);
                            bytes_read += n;

                            if let Some(ref sender) = self.progress {
                                // Truncation is acceptable: only used for progress display
                                #[allow(clippy::cast_possible_truncation)]
                                sender.send_lfs_progress(
                                    0,
                                    1,
                                    None,
                                    bytes_read,
                                    pointer.size as usize,
                                );
                            }
                        }

                        return Ok(content);
                    }

                    let status = response.status();
                    if Self::is_transient_status(status) && attempt + 1 < self.retry_max_attempts {
                        attempt += 1;
                        warn!(
                            attempt = attempt,
                            max_attempts = self.retry_max_attempts,
                            status = %status,
                            delay_ms = delay_ms,
                            "LFS download returned transient error, retrying"
                        );
                        std::thread::sleep(Duration::from_millis(delay_ms));
                        delay_ms = self.next_backoff(delay_ms);
                        continue;
                    }

                    return Err(Git2Error::Git2(format!(
                        "LFS download returned status {status}"
                    )));
                }
                Err(e) => {
                    if Self::is_transient_error(&e) && attempt + 1 < self.retry_max_attempts {
                        attempt += 1;
                        warn!(
                            attempt = attempt,
                            max_attempts = self.retry_max_attempts,
                            error = %e,
                            delay_ms = delay_ms,
                            "LFS download failed with transient error, retrying"
                        );
                        std::thread::sleep(Duration::from_millis(delay_ms));
                        delay_ms = self.next_backoff(delay_ms);
                        continue;
                    }

                    return Err(Git2Error::Git2(format!("LFS download failed: {e}")));
                }
            }
        }
    }

    /// Batch fetch multiple LFS objects.
    ///
    /// This is more efficient than calling `fetch_content` multiple times
    /// as it uses a single Batch API request. Objects that exceed size
    /// limits are skipped with a warning.
    ///
    /// # Arguments
    ///
    /// * `pointers` - List of LFS pointers to fetch
    ///
    /// # Returns
    ///
    /// An `LfsBatchResult` containing the fetched content and skip counts.
    ///
    /// # Errors
    ///
    /// Returns `Git2Error` if the batch API request fails or returns an error.
    #[allow(clippy::too_many_lines)] // Batch fetch with size checks is naturally verbose
    pub fn fetch_batch(&self, pointers: &[LfsPointer]) -> Result<LfsBatchResult, Git2Error> {
        if pointers.is_empty() {
            return Ok(LfsBatchResult {
                contents: HashMap::new(),
                skipped_too_large: 0,
            });
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

        let response = self.post_with_retry(&batch_url, &headers, &request)?;

        let batch_response: LfsBatchResponse = response
            .json()
            .map_err(|e| Git2Error::Git2(format!("failed to parse LFS response: {e}")))?;

        // Download each object
        let mut results = HashMap::new();
        let mut skipped_too_large = 0usize;
        let mut cumulative_bytes = 0u64;
        let total_objects = batch_response.objects.len();

        for (index, obj) in batch_response.objects.into_iter().enumerate() {
            if obj.error.is_some() {
                warn!(oid = %obj.oid, "LFS object has error, skipping");
                continue;
            }

            // Check per-object size limit
            if let Some(max_size) = self.max_object_size {
                if obj.size > max_size {
                    warn!(
                        oid = %obj.oid,
                        size = obj.size,
                        max_object_size = max_size,
                        "LFS object exceeds max_object_size, skipping"
                    );
                    skipped_too_large += 1;
                    continue;
                }
            }

            // Check cumulative total size limit
            if let Some(max_total) = self.max_total_size {
                if cumulative_bytes + obj.size > max_total {
                    warn!(
                        oid = %obj.oid,
                        size = obj.size,
                        cumulative = cumulative_bytes,
                        max_total_size = max_total,
                        "LFS total size would exceed max_total_size, skipping"
                    );
                    skipped_too_large += 1;
                    continue;
                }
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

            // Download content with retry
            match self.download_with_retry(&download.href, &download_headers) {
                Ok(content) => {
                    cumulative_bytes += content.len() as u64;
                    trace!(oid = %obj.oid, size = content.len(), "downloaded LFS object");
                    results.insert(obj.oid, content);

                    // Report per-file progress
                    if let Some(ref sender) = self.progress {
                        sender.send_lfs_progress(index + 1, total_objects, None, 0, 0);
                    }
                }
                Err(e) => {
                    warn!(oid = %obj.oid, error = %e, "LFS download failed");
                }
            }
        }

        debug!(
            requested = pointers.len(),
            fetched = results.len(),
            skipped_too_large = skipped_too_large,
            "batch fetch complete"
        );

        Ok(LfsBatchResult {
            contents: results,
            skipped_too_large,
        })
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
        let config = LfsConfig::default();
        let client = LfsClient::new(
            "https://github.com/owner/repo.git",
            None,
            None,
            None,
            &config,
            None,
        );
        assert!(client.is_ok());
    }

    #[test]
    fn lfs_client_with_credentials() {
        let config = LfsConfig::default();
        let client = LfsClient::new(
            "https://github.com/owner/repo.git",
            Some(("user".to_string(), "pass".to_string())),
            None,
            None,
            &config,
            None,
        );
        assert!(client.is_ok());
    }

    #[test]
    fn lfs_client_with_custom_config() {
        let config = LfsConfig {
            retry_max_attempts: 5,
            retry_initial_backoff_ms: 1000,
            retry_max_backoff_ms: 60_000,
            retry_backoff_multiplier: 3.0,
            max_object_size: Some(100 * 1024 * 1024),
            max_total_size: Some(500 * 1024 * 1024),
        };
        let client = LfsClient::new(
            "https://github.com/owner/repo.git",
            None,
            None,
            None,
            &config,
            None,
        );
        assert!(client.is_ok());
        let client = client.unwrap();
        assert_eq!(client.retry_max_attempts, 5);
        assert_eq!(client.retry_initial_backoff_ms, 1000);
        assert_eq!(client.max_object_size, Some(100 * 1024 * 1024));
        assert_eq!(client.max_total_size, Some(500 * 1024 * 1024));
    }

    #[test]
    fn is_transient_status_identifies_retryable_codes() {
        assert!(LfsClient::is_transient_status(
            reqwest::StatusCode::TOO_MANY_REQUESTS
        ));
        assert!(LfsClient::is_transient_status(
            reqwest::StatusCode::INTERNAL_SERVER_ERROR
        ));
        assert!(LfsClient::is_transient_status(
            reqwest::StatusCode::BAD_GATEWAY
        ));
        assert!(LfsClient::is_transient_status(
            reqwest::StatusCode::SERVICE_UNAVAILABLE
        ));
        assert!(LfsClient::is_transient_status(
            reqwest::StatusCode::GATEWAY_TIMEOUT
        ));
    }

    #[test]
    fn is_transient_status_rejects_client_errors() {
        assert!(!LfsClient::is_transient_status(
            reqwest::StatusCode::UNAUTHORIZED
        ));
        assert!(!LfsClient::is_transient_status(
            reqwest::StatusCode::FORBIDDEN
        ));
        assert!(!LfsClient::is_transient_status(
            reqwest::StatusCode::NOT_FOUND
        ));
        assert!(!LfsClient::is_transient_status(
            reqwest::StatusCode::BAD_REQUEST
        ));
    }
}
