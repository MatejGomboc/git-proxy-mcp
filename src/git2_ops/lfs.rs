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
//! - LFS server communication uses whatever scheme the repository URL
//!   declares: `https://` repos and `git@` SSH URLs (which we rewrite
//!   to `https://` for the LFS endpoint) communicate over HTTPS;
//!   `http://` repos communicate over HTTP. SSH-only LFS transports
//!   (the `lfs+ssh://` scheme) are not supported.

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
use crate::util::sanitize_for_log;

/// Maximum size of an LFS pointer file (per spec).
const MAX_POINTER_SIZE: usize = 1024;

/// LFS pointer file version identifier.
const LFS_POINTER_VERSION: &str = "https://git-lfs.github.com/spec/v1";

/// Expected first line of an LFS pointer file (without the trailing newline).
///
/// `is_lfs_pointer` matches the entire first line against this so a
/// hypothetical future `spec/v10` URL doesn't accidentally pass the `v1`
/// check (which `starts_with` would have allowed).
const LFS_POINTER_VERSION_LINE: &str = "version https://git-lfs.github.com/spec/v1";

/// User-agent for outbound HTTP requests, picked up from the crate version
/// at compile time so it never drifts from `Cargo.toml`.
const USER_AGENT: &str = concat!("git-proxy-mcp/", env!("CARGO_PKG_VERSION"));

/// Cap on the initial `Vec::with_capacity` for a download buffer.
///
/// The pointer can claim an arbitrary `size` (the LFS server is only
/// half-trusted: even an honest server may have stale or wrong metadata),
/// so allocating `pointer.size` up-front would let a hostile or buggy
/// pointer crash the process via OOM. We pre-allocate up to 16 MiB and
/// let the `Vec` grow on demand beyond that — the actual-size cap below
/// stops growth from running away.
const INITIAL_DOWNLOAD_CAPACITY_CAP: usize = 16 * 1024 * 1024;

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
/// Quick check without full parsing — used to filter candidates. The
/// match is on the *complete* first line (`lines()` strips trailing
/// `\n` and `\r\n`) rather than `starts_with`, so a hypothetical future
/// `spec/v10` URL doesn't accidentally match this `v1` check.
#[must_use]
pub fn is_lfs_pointer(content: &[u8]) -> bool {
    // Must be small enough
    if content.len() > MAX_POINTER_SIZE {
        return false;
    }

    // Must be valid UTF-8 and have the exact version line as line 1.
    let Ok(text) = std::str::from_utf8(content) else {
        return false;
    };
    text.lines().next() == Some(LFS_POINTER_VERSION_LINE)
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
    /// Per-object download timeout. Object downloads can take far longer
    /// than the Batch API call (multi-GiB blobs, slow CDNs), so the
    /// download GET gets its own cap, applied via `RequestBuilder::timeout`.
    download_timeout: Duration,
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

        if credentials.is_none() {
            warn!(
                lfs_url = %lfs_url,
                "LFS client created without credentials — batch API requests to private repos will likely return 401/403"
            );
        } else {
            debug!(lfs_url = %lfs_url, "LFS client created with credentials");
        }

        let mut builder = Client::builder()
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(lfs_config.request_timeout_secs))
            .connect_timeout(Duration::from_secs(lfs_config.connect_timeout_secs));

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
            download_timeout: Duration::from_secs(lfs_config.download_timeout_secs),
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

    /// Downloads content from a URL with retry, chunked reads, an actual-byte
    /// size cap, and optional progress reporting.
    ///
    /// Only transient errors (HTTP 429, 500, 502, 503, 504, and
    /// connection/timeout errors) are retried. Client errors such as 401,
    /// 403, 404 are not.
    ///
    /// # Why chunked reads
    ///
    /// `read_to_end` would let a malicious or buggy server send arbitrarily
    /// many bytes (the `Content-Length` header is server-controlled and the
    /// pre-flight `pointer.size` check uses metadata that may not match the
    /// actual response). Reading in fixed-size chunks lets us bail as soon
    /// as the byte counter exceeds [`Self::max_object_size`], capping memory
    /// growth at one chunk past the limit.
    ///
    /// # Arguments
    ///
    /// * `url` — download URL from the LFS Batch API.
    /// * `headers` — server-supplied download headers (e.g. signed-URL auth).
    /// * `pointer` — the pointer being downloaded. `pointer.size` is the
    ///   *expected* total used for progress percentages and as a hint for
    ///   initial buffer capacity. The actual-byte cap is enforced
    ///   regardless of what the pointer claimed.
    ///
    /// The per-object [`Self::download_timeout`] is applied via
    /// `RequestBuilder::timeout`, overriding the `Client`-level default
    /// (which targets the much shorter Batch API POST).
    fn download_chunked(
        &self,
        url: &str,
        headers: &HeaderMap,
        pointer: &LfsPointer,
    ) -> Result<Vec<u8>, Git2Error> {
        let mut attempt = 0u32;
        let mut delay_ms = self.retry_initial_backoff_ms;

        loop {
            let result = self
                .client
                .get(url)
                .timeout(self.download_timeout)
                .headers(headers.clone())
                .send();

            match result {
                Ok(mut response) => {
                    if response.status().is_success() {
                        return self.read_response_body(&mut response, pointer);
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

    /// Reads a successful HTTP response body in fixed-size chunks, enforcing
    /// the actual-byte cap and (optionally) emitting progress notifications.
    ///
    /// Split out from [`Self::download_chunked`] so the retry loop stays
    /// readable. The pointer is required (not `Option`) because the only
    /// caller (`download_chunked`, in turn called from `fetch_content`)
    /// always knows which object it's downloading.
    #[allow(clippy::cast_possible_truncation)] // see capacity comment
    fn read_response_body(
        &self,
        response: &mut reqwest::blocking::Response,
        pointer: &LfsPointer,
    ) -> Result<Vec<u8>, Git2Error> {
        // Pre-allocate at most the smaller of:
        //   - the declared `pointer.size` (so we don't reserve more than
        //     we expect to receive)
        //   - `max_object_size` if configured (so a server lying with a
        //     huge declared size can't make us reserve more than the
        //     operator allowed in the first place)
        //   - INITIAL_DOWNLOAD_CAPACITY_CAP (16 MiB safety bound for the
        //     unlimited-`max_object_size` case, so a hostile pointer
        //     claiming `u64::MAX` can't OOM us via Vec::with_capacity
        //     before reading even starts)
        // The Vec still grows on demand past the initial allocation, but
        // the actual-byte cap inside the read loop stops growth from
        // running away.
        let cap_from_config = self
            .max_object_size
            .map_or(INITIAL_DOWNLOAD_CAPACITY_CAP as u64, |m| {
                m.min(INITIAL_DOWNLOAD_CAPACITY_CAP as u64)
            });
        let initial_capacity = pointer.size.min(cap_from_config) as usize;
        let mut content = Vec::with_capacity(initial_capacity);
        let mut buf = vec![0u8; LFS_DOWNLOAD_CHUNK_SIZE];
        let mut bytes_read: u64 = 0;

        loop {
            let n = response
                .read(&mut buf)
                .map_err(|e| Git2Error::Git2(format!("failed to read LFS content: {e}")))?;
            if n == 0 {
                break;
            }
            bytes_read = bytes_read.saturating_add(n as u64);

            // Enforce the actual-download cap. The pre-flight check on
            // pointer.size is necessary but not sufficient: a hostile or
            // buggy server can return more bytes than it declared.
            if let Some(max_size) = self.max_object_size {
                if bytes_read > max_size {
                    return Err(Git2Error::Git2(format!(
                        "LFS object {oid} exceeded max_object_size during download \
                         ({bytes_read} > {max_size} bytes)",
                        oid = pointer.oid
                    )));
                }
            }

            content.extend_from_slice(&buf[..n]);

            if let Some(ref sender) = self.progress {
                // Truncation is acceptable: only used for progress display.
                #[allow(clippy::cast_possible_truncation)]
                sender.send_lfs_progress(0, 1, None, bytes_read as usize, pointer.size as usize);
            }
        }

        Ok(content)
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

                    // Non-retryable failure: capture the response body so the
                    // operator can see what the LFS server actually said
                    // (e.g. "Bad credentials", "Repository not found", rate
                    // limit details). The body is server-generated text — it
                    // does not echo our Authorization header. We sanitise
                    // before logging or surfacing in the error so that a
                    // hostile or buggy server can't inject ANSI escapes or
                    // fake newlines into the operator's terminal log.
                    let body_text = response
                        .text()
                        .unwrap_or_else(|e| format!("<failed to read response body: {e}>"));
                    let body_text = sanitize_for_log(&body_text);
                    warn!(
                        status = %status,
                        url = %url,
                        response_body = %body_text,
                        "LFS batch POST returned non-retryable error status"
                    );
                    return Err(Git2Error::Git2(format!(
                        "LFS batch API returned status {status}: {body_text}"
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

        // Add Basic auth if credentials are available. Propagate the
        // (in-practice unreachable) `HeaderValue::from_str` failure
        // rather than dropping the header silently — silent fallback to
        // an anonymous request would surface as a confusing 401 instead
        // of the actual encoding error.
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

        // Check for error. The message is server-controlled, so sanitise
        // it before letting it through to logs/output.
        if let Some(err) = obj.error {
            return Err(Git2Error::Git2(format!(
                "LFS error: {}",
                sanitize_for_log(&err.message)
            )));
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

        // Always read in chunks so we can enforce max_object_size against
        // the actual response body (server-supplied content can exceed
        // pointer.size). Progress is reported only when a sender is set.
        let content = self.download_chunked(&download.href, &download_headers, pointer)?;

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
}

/// Derive LFS server URL from git repository URL.
///
/// The LFS endpoint is `<repo_url>/info/lfs` with the repo URL preserved
/// verbatim — including any `.git` suffix. GitHub in particular requires
/// the `.git` to remain (e.g. `https://github.com/o/r.git/info/lfs/...`);
/// stripping it routes the request to the web frontend, which returns a
/// 422 + HTML page instead of an LFS batch JSON response. This matches
/// the canonical `git-lfs` client behaviour.
///
/// Validates that:
/// - SSH URLs have both a non-empty host and a non-empty path component
///   (`git@host:path`). Without this check, `git@:repo.git` rewrites to
///   `https:///repo.git` (no host) and `git@host` rewrites to
///   `https://host` (no repo path) — both produce useless requests that
///   leak the malformed URL into error logs.
/// - HTTP(S) URLs have a non-empty host (`https://host/...`). `https:///x`
///   has an empty host and would otherwise be passed through.
fn derive_lfs_url(repo_url: &str) -> Result<String, Git2Error> {
    // Handle SSH URLs: git@github.com:owner/repo.git -> https://github.com/owner/repo.git
    let https_url = if let Some(rest) = repo_url.strip_prefix("git@") {
        let (host, path) = rest
            .split_once(':')
            .ok_or_else(|| Git2Error::Git2("invalid SSH URL: missing ':' separator".to_string()))?;
        if host.is_empty() {
            return Err(Git2Error::Git2(
                "invalid SSH URL: empty host before ':'".to_string(),
            ));
        }
        if path.is_empty() {
            return Err(Git2Error::Git2(
                "invalid SSH URL: empty path after ':'".to_string(),
            ));
        }
        format!("https://{host}/{path}")
    } else if let Some(rest) = repo_url
        .strip_prefix("https://")
        .or_else(|| repo_url.strip_prefix("http://"))
    {
        // After the scheme, the next character must NOT be `/` (which
        // would mean an empty host, e.g. `https:///path`) and `rest` must
        // not be empty.
        if rest.is_empty() || rest.starts_with('/') {
            return Err(Git2Error::Git2(
                "invalid HTTP(S) URL: empty host".to_string(),
            ));
        }
        repo_url.to_string()
    } else {
        return Err(Git2Error::Git2(format!(
            "unsupported URL scheme: {repo_url}"
        )));
    };

    // LFS endpoint — preserve any `.git` suffix in https_url.
    Ok(format!("{https_url}/info/lfs"))
}

#[cfg(test)]
#[allow(clippy::significant_drop_tightening)] // mockito::Server must outlive LfsClient calls
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
        // The `.git` suffix must be preserved — GitHub returns 422 + HTML
        // if we strip it.
        let url = derive_lfs_url("https://github.com/owner/repo.git").unwrap();
        assert_eq!(url, "https://github.com/owner/repo.git/info/lfs");
    }

    #[test]
    fn derive_lfs_url_https_no_git_suffix() {
        // Already lacks `.git` — passed through verbatim. The user is
        // responsible for providing a URL their LFS server accepts.
        let url = derive_lfs_url("https://github.com/owner/repo").unwrap();
        assert_eq!(url, "https://github.com/owner/repo/info/lfs");
    }

    #[test]
    fn derive_lfs_url_ssh() {
        // SSH `.git` is preserved through the SSH-to-HTTPS rewrite.
        let url = derive_lfs_url("git@github.com:owner/repo.git").unwrap();
        assert_eq!(url, "https://github.com/owner/repo.git/info/lfs");
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
            request_timeout_secs: 600,
            connect_timeout_secs: 60,
            download_timeout_secs: 1200,
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

    #[test]
    fn is_transient_status_rejects_2xx() {
        assert!(!LfsClient::is_transient_status(reqwest::StatusCode::OK));
        assert!(!LfsClient::is_transient_status(
            reqwest::StatusCode::CREATED
        ));
        assert!(!LfsClient::is_transient_status(
            reqwest::StatusCode::NO_CONTENT
        ));
    }

    #[test]
    fn is_transient_status_rejects_3xx() {
        assert!(!LfsClient::is_transient_status(
            reqwest::StatusCode::MOVED_PERMANENTLY
        ));
        assert!(!LfsClient::is_transient_status(reqwest::StatusCode::FOUND));
    }

    #[test]
    fn is_transient_status_only_accepts_specific_5xx_codes() {
        // Implementation only retries 500, 502, 503, 504 — not all 5xx
        assert!(LfsClient::is_transient_status(
            reqwest::StatusCode::INTERNAL_SERVER_ERROR
        ));
        assert!(!LfsClient::is_transient_status(
            reqwest::StatusCode::NOT_IMPLEMENTED // 501 — not retryable
        ));
        assert!(!LfsClient::is_transient_status(
            reqwest::StatusCode::HTTP_VERSION_NOT_SUPPORTED // 505 — not retryable
        ));
    }

    #[test]
    fn lfs_pointer_struct_fields() {
        let pointer = LfsPointer {
            oid: "abc123".to_string(),
            size: 1024,
        };
        assert_eq!(pointer.oid, "abc123");
        assert_eq!(pointer.size, 1024);
        // Verify Clone works
        let cloned = pointer.clone();
        assert_eq!(cloned.oid, pointer.oid);
    }

    #[test]
    fn parse_lfs_pointer_with_extra_fields_is_robust() {
        let pointer = b"version https://git-lfs.github.com/spec/v1\n\
                        oid sha256:abc123def456\n\
                        size 999\n\
                        extra-field something\n";
        let parsed = parse_lfs_pointer(pointer);
        assert!(parsed.is_some());
        let p = parsed.unwrap();
        assert_eq!(p.oid, "abc123def456");
        assert_eq!(p.size, 999);
    }

    #[test]
    fn parse_lfs_pointer_invalid_size() {
        let pointer = b"version https://git-lfs.github.com/spec/v1\n\
                        oid sha256:abc\n\
                        size not_a_number\n";
        assert!(parse_lfs_pointer(pointer).is_none());
    }

    #[test]
    fn parse_lfs_pointer_zero_size() {
        let pointer = b"version https://git-lfs.github.com/spec/v1\n\
                        oid sha256:abc\n\
                        size 0\n";
        let parsed = parse_lfs_pointer(pointer).unwrap();
        assert_eq!(parsed.size, 0);
    }

    #[test]
    fn parse_lfs_pointer_oid_without_sha256_prefix() {
        // OID line without sha256: prefix should be rejected
        let pointer = b"version https://git-lfs.github.com/spec/v1\n\
                        oid abc123\n\
                        size 100\n";
        assert!(parse_lfs_pointer(pointer).is_none());
    }

    #[test]
    fn parse_lfs_pointer_empty_input() {
        assert!(parse_lfs_pointer(b"").is_none());
    }

    #[test]
    fn parse_lfs_pointer_missing_version_line() {
        let pointer = b"oid sha256:abc\nsize 100\n";
        assert!(parse_lfs_pointer(pointer).is_none());
    }

    #[test]
    fn is_lfs_pointer_empty_input() {
        assert!(!is_lfs_pointer(b""));
    }

    #[test]
    fn is_lfs_pointer_partial_match() {
        // Has "version" but not the LFS spec URL
        let content = b"version 1.0\n";
        assert!(!is_lfs_pointer(content));
    }

    #[test]
    fn derive_lfs_url_http() {
        let url = derive_lfs_url("http://example.com/owner/repo.git").unwrap();
        assert_eq!(url, "http://example.com/owner/repo.git/info/lfs");
    }

    #[test]
    fn derive_lfs_url_ssh_no_git_suffix() {
        let url = derive_lfs_url("git@gitlab.com:group/project").unwrap();
        assert_eq!(url, "https://gitlab.com/group/project/info/lfs");
    }

    #[test]
    fn derive_lfs_url_ssh_self_hosted() {
        let url = derive_lfs_url("git@gitlab.example.com:group/project.git").unwrap();
        assert_eq!(url, "https://gitlab.example.com/group/project.git/info/lfs");
    }

    #[test]
    fn derive_lfs_url_rejects_unknown_scheme() {
        assert!(derive_lfs_url("ftp://example.com/repo").is_err());
        assert!(derive_lfs_url("gopher://example.com/repo").is_err());
    }

    #[test]
    fn derive_lfs_url_rejects_no_scheme() {
        assert!(derive_lfs_url("example.com/repo").is_err());
    }

    #[test]
    fn lfs_client_with_progress() {
        let (sender, _receiver) = crate::mcp::progress::ProgressSender::new("t".to_string());
        let config = LfsConfig::default();
        let client = LfsClient::new(
            "https://github.com/owner/repo.git",
            None,
            None,
            None,
            &config,
            Some(sender),
        );
        assert!(client.is_ok());
    }

    #[test]
    fn lfs_client_invalid_repo_url() {
        let config = LfsConfig::default();
        let client = LfsClient::new("ftp://invalid.com/repo", None, None, None, &config, None);
        assert!(client.is_err());
    }

    // ------------------------------------------------------------------
    // Mock LFS server tests
    //
    // These spin up a `mockito` HTTP server, configure an `LfsClient`
    // pointing at it, and exercise the retry/error-mapping paths that are
    // otherwise only reached by talking to a real LFS endpoint.
    //
    // The `lfs_url` is derived from the repo_url as `{repo_url}/info/lfs`
    // (the `.git` suffix is preserved verbatim — see `derive_lfs_url` doc),
    // so we pass `repo_url = "{mock_server}/repo.git"` and the derived batch
    // endpoint is `{mock_server}/repo.git/info/lfs/objects/batch`.
    // ------------------------------------------------------------------

    /// Build a fast-retry config so transient failures don't stall the test.
    fn fast_retry_config(attempts: u32) -> LfsConfig {
        LfsConfig {
            retry_max_attempts: attempts,
            retry_initial_backoff_ms: 1,
            retry_max_backoff_ms: 5,
            retry_backoff_multiplier: 2.0,
            max_object_size: None,
            request_timeout_secs: 30,
            connect_timeout_secs: 5,
            download_timeout_secs: 30,
        }
    }

    fn make_pointer(oid: &str, size: u64) -> LfsPointer {
        LfsPointer {
            oid: oid.to_string(),
            size,
        }
    }

    fn make_batch_response(oid: &str, size: u64, download_href: &str) -> String {
        format!(
            r#"{{
                "objects": [
                    {{
                        "oid": "{oid}",
                        "size": {size},
                        "actions": {{
                            "download": {{
                                "href": "{download_href}",
                                "header": {{}}
                            }}
                        }}
                    }}
                ]
            }}"#
        )
    }

    #[test]
    fn fetch_content_succeeds_against_mock_server() {
        let mut server = mockito::Server::new();
        let oid = "abc123def4567890abc123def4567890abc123def4567890abc123def4567890";
        let payload = b"hello LFS world";
        let download_path = format!("/repo.git/info/lfs/objects/{oid}");
        let download_href = format!("{}{}", server.url(), download_path);

        let _batch_mock = server
            .mock("POST", "/repo.git/info/lfs/objects/batch")
            .with_status(200)
            .with_header("content-type", "application/vnd.git-lfs+json")
            .with_body(make_batch_response(
                oid,
                payload.len() as u64,
                &download_href,
            ))
            .create();

        let _download_mock = server
            .mock("GET", download_path.as_str())
            .with_status(200)
            .with_body(payload)
            .create();

        let repo_url = format!("{}/repo.git", server.url());
        let config = fast_retry_config(3);
        let client = LfsClient::new(&repo_url, None, None, None, &config, None).unwrap();

        let pointer = make_pointer(oid, payload.len() as u64);
        let content = client.fetch_content(&pointer).unwrap();
        assert_eq!(content, payload);
    }

    #[test]
    fn fetch_content_retries_on_transient_5xx_then_succeeds() {
        let mut server = mockito::Server::new();
        let oid = "abc123def4567890abc123def4567890abc123def4567890abc123def4567890";
        let payload = b"recovered after retry";
        let download_path = format!("/repo.git/info/lfs/objects/{oid}");
        let download_href = format!("{}{}", server.url(), download_path);

        // First attempt: 503 Service Unavailable (transient)
        let _failing_mock = server
            .mock("POST", "/repo.git/info/lfs/objects/batch")
            .with_status(503)
            .expect(1)
            .create();
        // Second attempt: success
        let _success_mock = server
            .mock("POST", "/repo.git/info/lfs/objects/batch")
            .with_status(200)
            .with_header("content-type", "application/vnd.git-lfs+json")
            .with_body(make_batch_response(
                oid,
                payload.len() as u64,
                &download_href,
            ))
            .expect(1)
            .create();

        let _download_mock = server
            .mock("GET", download_path.as_str())
            .with_status(200)
            .with_body(payload)
            .create();

        let repo_url = format!("{}/repo.git", server.url());
        let config = fast_retry_config(5);
        let client = LfsClient::new(&repo_url, None, None, None, &config, None).unwrap();

        let pointer = make_pointer(oid, payload.len() as u64);
        let content = client.fetch_content(&pointer).unwrap();
        assert_eq!(content, payload);
    }

    #[test]
    fn fetch_content_does_not_retry_on_4xx() {
        let mut server = mockito::Server::new();
        let oid = "abc123def4567890abc123def4567890abc123def4567890abc123def4567890";

        // 401 Unauthorized — not retryable.
        let _mock = server
            .mock("POST", "/repo.git/info/lfs/objects/batch")
            .with_status(401)
            .expect(1) // Must only be called once.
            .create();

        let repo_url = format!("{}/repo.git", server.url());
        let config = fast_retry_config(5);
        let client = LfsClient::new(&repo_url, None, None, None, &config, None).unwrap();

        let pointer = make_pointer(oid, 100);
        let result = client.fetch_content(&pointer);
        assert!(result.is_err());
    }

    #[test]
    fn fetch_content_gives_up_after_max_retries_on_persistent_5xx() {
        let mut server = mockito::Server::new();
        let oid = "abc123def4567890abc123def4567890abc123def4567890abc123def4567890";
        let max_attempts = 3;

        // All requests return 502 Bad Gateway (transient).
        let _mock = server
            .mock("POST", "/repo.git/info/lfs/objects/batch")
            .with_status(502)
            .expect(max_attempts as usize)
            .create();

        let repo_url = format!("{}/repo.git", server.url());
        let config = fast_retry_config(max_attempts);
        let client = LfsClient::new(&repo_url, None, None, None, &config, None).unwrap();

        let pointer = make_pointer(oid, 100);
        let result = client.fetch_content(&pointer);
        assert!(result.is_err());
    }

    #[test]
    fn fetch_content_rejects_oversized_object_before_request() {
        let mut server = mockito::Server::new();
        let oid = "abc123def4567890abc123def4567890abc123def4567890abc123def4567890";

        // No mock should be reached — the size check fails first.
        let _mock = server
            .mock("POST", "/repo.git/info/lfs/objects/batch")
            .with_status(200)
            .expect(0) // Must NOT be called.
            .create();

        let repo_url = format!("{}/repo.git", server.url());
        let config = LfsConfig {
            retry_max_attempts: 3,
            retry_initial_backoff_ms: 1,
            retry_max_backoff_ms: 5,
            retry_backoff_multiplier: 2.0,
            max_object_size: Some(50),
            request_timeout_secs: 30,
            connect_timeout_secs: 5,
            download_timeout_secs: 30,
        };
        let client = LfsClient::new(&repo_url, None, None, None, &config, None).unwrap();

        let pointer = make_pointer(oid, 1_000_000);
        let result = client.fetch_content(&pointer);
        assert!(result.is_err());
        let msg = format!("{}", result.unwrap_err());
        assert!(msg.contains("max_object_size"));
    }

    #[test]
    fn fetch_content_returns_error_when_lfs_response_has_error_field() {
        let mut server = mockito::Server::new();
        let oid = "abc123def4567890abc123def4567890abc123def4567890abc123def4567890";

        let response_body = format!(
            r#"{{
                "objects": [
                    {{
                        "oid": "{oid}",
                        "size": 100,
                        "error": {{
                            "code": 404,
                            "message": "Object does not exist on the server"
                        }}
                    }}
                ]
            }}"#
        );

        let _mock = server
            .mock("POST", "/repo.git/info/lfs/objects/batch")
            .with_status(200)
            .with_header("content-type", "application/vnd.git-lfs+json")
            .with_body(response_body)
            .create();

        let repo_url = format!("{}/repo.git", server.url());
        let config = fast_retry_config(3);
        let client = LfsClient::new(&repo_url, None, None, None, &config, None).unwrap();

        let pointer = make_pointer(oid, 100);
        let result = client.fetch_content(&pointer);
        assert!(result.is_err());
        let msg = format!("{}", result.unwrap_err());
        assert!(msg.contains("Object does not exist") || msg.contains("LFS error"));
    }

    #[test]
    fn fetch_content_sends_basic_auth_when_credentials_set() {
        let mut server = mockito::Server::new();
        let oid = "abc123def4567890abc123def4567890abc123def4567890abc123def4567890";
        let payload = b"authenticated content";
        let download_path = format!("/repo.git/info/lfs/objects/{oid}");
        let download_href = format!("{}{}", server.url(), download_path);

        // Expect the Authorization header to be present (Basic base64(user:pass)).
        // base64(test-user:s3cret) = dGVzdC11c2VyOnMzY3JldA==
        let _batch_mock = server
            .mock("POST", "/repo.git/info/lfs/objects/batch")
            .match_header("authorization", "Basic dGVzdC11c2VyOnMzY3JldA==")
            .with_status(200)
            .with_header("content-type", "application/vnd.git-lfs+json")
            .with_body(make_batch_response(
                oid,
                payload.len() as u64,
                &download_href,
            ))
            .create();

        let _download_mock = server
            .mock("GET", download_path.as_str())
            .with_status(200)
            .with_body(payload)
            .create();

        let repo_url = format!("{}/repo.git", server.url());
        let config = fast_retry_config(3);
        let creds = Some(("test-user".to_string(), "s3cret".to_string()));
        let client = LfsClient::new(&repo_url, creds, None, None, &config, None).unwrap();

        let pointer = make_pointer(oid, payload.len() as u64);
        let content = client.fetch_content(&pointer).unwrap();
        assert_eq!(content, payload);
    }

    // ------------------------------------------------------------------
    // Regression tests for the deep audit (PR #159)
    // ------------------------------------------------------------------

    #[test]
    fn is_lfs_pointer_does_not_match_hypothetical_future_v10() {
        // The original `starts_with("...spec/v1")` check would silently
        // accept `spec/v10`, `spec/v11`, etc. because v1 is a prefix of v10.
        // The line-exact match rejects them so a future spec bump doesn't
        // get misclassified as v1.
        let v10 = b"version https://git-lfs.github.com/spec/v10\n\
                    oid sha256:abc\n\
                    size 100\n";
        assert!(!is_lfs_pointer(v10));

        // Also reject `v1foo` and other suffix-style variants.
        let v1_with_suffix = b"version https://git-lfs.github.com/spec/v1-extended\n";
        assert!(!is_lfs_pointer(v1_with_suffix));
    }

    #[test]
    fn is_lfs_pointer_accepts_crlf_line_ending() {
        // `lines()` handles `\r\n` as well as `\n`. A pointer file
        // committed from Windows should still classify correctly.
        let crlf = b"version https://git-lfs.github.com/spec/v1\r\n\
                     oid sha256:abc\r\n\
                     size 1\r\n";
        assert!(is_lfs_pointer(crlf));
    }

    #[test]
    fn derive_lfs_url_rejects_ssh_with_empty_host() {
        // `git@:repo.git` would have rewritten to `https:///repo.git`.
        let result = derive_lfs_url("git@:repo.git");
        assert!(result.is_err());
        let msg = format!("{}", result.unwrap_err());
        assert!(msg.contains("empty host"), "got: {msg}");
    }

    #[test]
    fn derive_lfs_url_rejects_ssh_without_colon() {
        // `git@host` (no `:`) would have rewritten to `https://host` (no path).
        let result = derive_lfs_url("git@host");
        assert!(result.is_err());
        let msg = format!("{}", result.unwrap_err());
        assert!(msg.contains("missing ':' separator"), "got: {msg}");
    }

    #[test]
    fn derive_lfs_url_rejects_ssh_with_empty_path() {
        // `git@host:` (colon but nothing after).
        let result = derive_lfs_url("git@host:");
        assert!(result.is_err());
        let msg = format!("{}", result.unwrap_err());
        assert!(msg.contains("empty path"), "got: {msg}");
    }

    #[test]
    fn derive_lfs_url_rejects_https_with_empty_host() {
        // `https:///path` would have been passed through verbatim.
        assert!(derive_lfs_url("https:///path").is_err());
        assert!(derive_lfs_url("https://").is_err());
        assert!(derive_lfs_url("http:///x").is_err());
    }

    #[test]
    fn fetch_content_rejects_oversize_actual_response() {
        // Pre-flight pointer.size check passes (under max_object_size),
        // but the server returns far more bytes than declared. The
        // actual-byte cap must catch this — without it, the server can
        // send arbitrary data even when max_object_size is configured.
        let mut server = mockito::Server::new();
        let oid = "abc123def4567890abc123def4567890abc123def4567890abc123def4567890";
        let download_path = format!("/repo.git/info/lfs/objects/{oid}");
        let download_href = format!("{}{}", server.url(), download_path);

        // Pointer claims 100 bytes; server actually sends 200 KiB.
        let actual_payload = vec![b'X'; 200 * 1024];

        let _batch_mock = server
            .mock("POST", "/repo.git/info/lfs/objects/batch")
            .with_status(200)
            .with_header("content-type", "application/vnd.git-lfs+json")
            .with_body(make_batch_response(oid, 100, &download_href))
            .create();
        let _download_mock = server
            .mock("GET", download_path.as_str())
            .with_status(200)
            .with_body(actual_payload)
            .create();

        let repo_url = format!("{}/repo.git", server.url());
        let config = LfsConfig {
            retry_max_attempts: 3,
            retry_initial_backoff_ms: 1,
            retry_max_backoff_ms: 5,
            retry_backoff_multiplier: 2.0,
            // Cap allows 100-byte declared object (passes pre-flight) but
            // not 200 KiB actual response (must fail mid-download).
            max_object_size: Some(1024),
            request_timeout_secs: 30,
            connect_timeout_secs: 5,
            download_timeout_secs: 30,
        };
        let client = LfsClient::new(&repo_url, None, None, None, &config, None).unwrap();

        let pointer = make_pointer(oid, 100);
        let result = client.fetch_content(&pointer);
        assert!(result.is_err(), "oversize actual response must be rejected");
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.contains("max_object_size"),
            "error must reference max_object_size, got: {msg}"
        );
    }

    #[test]
    fn fetch_content_sanitises_server_error_body_in_log() {
        // A non-retryable status (e.g. 400) returns the server body in
        // the error message. If the server inserts ANSI escapes or fake
        // newlines, they must be escaped, not rendered.
        let mut server = mockito::Server::new();
        let oid = "abc123def4567890abc123def4567890abc123def4567890abc123def4567890";

        // 400 is non-retryable; body contains an ANSI escape and a newline.
        let evil_body = "boom\x1b[31mFAKE\nLOG LINE\x1b[0m";
        let _mock = server
            .mock("POST", "/repo.git/info/lfs/objects/batch")
            .with_status(400)
            .with_body(evil_body)
            .create();

        let repo_url = format!("{}/repo.git", server.url());
        let config = fast_retry_config(3);
        let client = LfsClient::new(&repo_url, None, None, None, &config, None).unwrap();

        let pointer = make_pointer(oid, 100);
        let result = client.fetch_content(&pointer);
        assert!(result.is_err());
        let msg = format!("{}", result.unwrap_err());
        // The literal ESC byte must NOT appear in the surfaced error.
        assert!(
            !msg.contains('\x1b'),
            "raw ESC must be escaped in error msg"
        );
        // And neither must a raw newline (would let server forge log lines).
        assert!(
            !msg.contains('\n'),
            "raw newline must be escaped in error msg"
        );
    }

    #[test]
    fn fetch_content_sanitises_lfs_error_message() {
        // The LFS server can also return an error in the JSON response
        // body (200 OK + objects[].error). That message is server-supplied
        // text and must be sanitised before being surfaced.
        let mut server = mockito::Server::new();
        let oid = "abc123def4567890abc123def4567890abc123def4567890abc123def4567890";

        let evil_body = format!(
            r#"{{
                "objects": [
                    {{
                        "oid": "{oid}",
                        "size": 100,
                        "error": {{
                            "code": 404,
                            "message": "missing[31mEVIL\nfake-log[0m"
                        }}
                    }}
                ]
            }}"#
        );

        let _mock = server
            .mock("POST", "/repo.git/info/lfs/objects/batch")
            .with_status(200)
            .with_header("content-type", "application/vnd.git-lfs+json")
            .with_body(evil_body)
            .create();

        let repo_url = format!("{}/repo.git", server.url());
        let config = fast_retry_config(3);
        let client = LfsClient::new(&repo_url, None, None, None, &config, None).unwrap();

        let pointer = make_pointer(oid, 100);
        let result = client.fetch_content(&pointer);
        assert!(result.is_err());
        let msg = format!("{}", result.unwrap_err());
        assert!(
            !msg.contains('\x1b'),
            "raw ESC must be escaped in LFS error msg"
        );
        assert!(
            !msg.contains('\n'),
            "raw newline must be escaped in LFS error msg"
        );
    }

    #[test]
    fn user_agent_contains_crate_version() {
        // Documents the contract that the user-agent stays in lockstep
        // with the crate version. This catches the "0.1 hardcoded forever"
        // drift that triggered this audit.
        assert!(USER_AGENT.starts_with("git-proxy-mcp/"));
        assert!(USER_AGENT.contains(env!("CARGO_PKG_VERSION")));
    }

    // ------------------------------------------------------------------
    // Coverage gaps flagged by Codecov on PR #159
    //
    // Each of these targets a specific line range in lfs.rs that the
    // earlier audit tests didn't reach. Previously they only ran against
    // a real LFS endpoint or never at all.
    // ------------------------------------------------------------------

    #[test]
    fn parse_lfs_pointer_skips_blank_lines() {
        // Line 113: the `if line.is_empty() { continue; }` branch in
        // parse_lfs_pointer was never exercised because all the existing
        // valid-pointer tests had no blank lines. Real-world pointer
        // files often have a trailing blank line after the last field.
        let pointer = b"version https://git-lfs.github.com/spec/v1\n\
                        \n\
                        oid sha256:abc123def4567890\n\
                        \n\
                        size 42\n\
                        \n";
        let parsed = parse_lfs_pointer(pointer).unwrap();
        assert_eq!(parsed.oid, "abc123def4567890");
        assert_eq!(parsed.size, 42);
    }

    #[test]
    fn lfs_client_constructs_with_proxy_and_no_proxy() {
        // Lines 273-280: the proxy-URL branch of LfsClient::new was
        // never exercised — every other test passed `None` for proxy.
        // We can't trivially exercise the actual proxy traffic without
        // a real HTTP proxy, but we CAN exercise the builder path and
        // confirm it doesn't error.
        let config = LfsConfig::default();
        let client = LfsClient::new(
            "https://github.com/owner/repo.git",
            None,
            Some("http://proxy.internal:3128"),
            Some("localhost,127.0.0.1,*.example.internal"),
            &config,
            None,
        );
        assert!(client.is_ok(), "proxy URL with no_proxy must build cleanly");
    }

    #[test]
    fn lfs_client_constructs_with_proxy_only() {
        // Same line range — also exercise the no_proxy=None branch.
        let config = LfsConfig::default();
        let client = LfsClient::new(
            "https://github.com/owner/repo.git",
            None,
            Some("socks5://proxy.internal:1080"),
            None,
            &config,
            None,
        );
        assert!(
            client.is_ok(),
            "proxy URL without no_proxy must build cleanly"
        );
    }

    #[test]
    fn lfs_client_rejects_invalid_proxy_url() {
        // Line 274: the .map_err(|e| ...) on Proxy::all() — exercises
        // the error path when the proxy URL itself is malformed.
        let config = LfsConfig::default();
        let client = LfsClient::new(
            "https://github.com/owner/repo.git",
            None,
            // "not a url" lacks a scheme — reqwest::Proxy::all rejects it.
            Some("not a url"),
            None,
            &config,
            None,
        );
        let Err(err) = client else {
            panic!("malformed proxy URL must be rejected");
        };
        let msg = format!("{err}");
        assert!(msg.contains("invalid proxy URL"), "got: {msg}");
    }

    #[test]
    fn fetch_content_retries_download_get_on_transient_5xx_then_succeeds() {
        // Lines 372-388 / 405-407: the download-GET retry loop. The
        // existing `fetch_content_retries_on_transient_5xx_then_succeeds`
        // test only flexes the Batch API POST retry — the actual blob
        // download had no retry-loop coverage.
        let mut server = mockito::Server::new();
        let oid = "abc123def4567890abc123def4567890abc123def4567890abc123def4567890";
        let payload = b"blob content after download retry";
        let download_path = format!("/repo.git/info/lfs/objects/{oid}");
        let download_href = format!("{}{}", server.url(), download_path);

        // Batch API: succeeds first try.
        let _batch_mock = server
            .mock("POST", "/repo.git/info/lfs/objects/batch")
            .with_status(200)
            .with_header("content-type", "application/vnd.git-lfs+json")
            .with_body(make_batch_response(
                oid,
                payload.len() as u64,
                &download_href,
            ))
            .create();

        // Download GET: 503 first, then 200. mockito serves mocks in
        // creation order when multiple match, so order matters here.
        let _download_fail = server
            .mock("GET", download_path.as_str())
            .with_status(503)
            .expect(1)
            .create();
        let _download_ok = server
            .mock("GET", download_path.as_str())
            .with_status(200)
            .with_body(payload)
            .expect(1)
            .create();

        let repo_url = format!("{}/repo.git", server.url());
        let config = fast_retry_config(5);
        let client = LfsClient::new(&repo_url, None, None, None, &config, None).unwrap();

        let pointer = make_pointer(oid, payload.len() as u64);
        let content = client.fetch_content(&pointer).unwrap();
        assert_eq!(content, payload);
    }

    #[test]
    fn fetch_content_emits_progress_when_sender_configured() {
        // Lines 476-479: the progress-sender callback inside
        // read_response_body. Without a sender, the `if let Some(...)`
        // branch is skipped entirely, leaving those lines uncovered.
        let mut server = mockito::Server::new();
        let oid = "abc123def4567890abc123def4567890abc123def4567890abc123def4567890";
        // Use a payload >= LFS_DOWNLOAD_CHUNK_SIZE so the read loop
        // makes multiple iterations and the progress callback fires
        // more than once. 96 KiB = 1.5 chunks.
        let payload = vec![b'P'; 96 * 1024];
        let download_path = format!("/repo.git/info/lfs/objects/{oid}");
        let download_href = format!("{}{}", server.url(), download_path);

        let _batch_mock = server
            .mock("POST", "/repo.git/info/lfs/objects/batch")
            .with_status(200)
            .with_header("content-type", "application/vnd.git-lfs+json")
            .with_body(make_batch_response(
                oid,
                payload.len() as u64,
                &download_href,
            ))
            .create();
        let _download_mock = server
            .mock("GET", download_path.as_str())
            .with_status(200)
            .with_body(&payload)
            .create();

        let repo_url = format!("{}/repo.git", server.url());
        let config = fast_retry_config(3);
        let (sender, receiver) =
            crate::mcp::progress::ProgressSender::new("test-token".to_string());
        let client = LfsClient::new(&repo_url, None, None, None, &config, Some(sender)).unwrap();

        let pointer = make_pointer(oid, payload.len() as u64);
        let content = client.fetch_content(&pointer).unwrap();
        assert_eq!(content.len(), payload.len());

        // Drain the channel; we expect at least one LfsDownload progress
        // update to have been emitted. (The exact count depends on the
        // 100-ms rate-limit in ProgressSender; one is the floor.)
        let mut lfs_updates = 0;
        while let Ok(update) = receiver.try_recv() {
            if matches!(
                update,
                crate::mcp::progress::ProgressUpdate::LfsDownload { .. }
            ) {
                lfs_updates += 1;
            }
        }
        assert!(
            lfs_updates >= 1,
            "expected at least one LfsDownload progress update, got 0"
        );
    }

    #[test]
    fn fetch_content_passes_through_server_supplied_download_headers() {
        // Lines 675-680: the loop that copies server-supplied download
        // headers into the GET request. Existing tests use `header: {}`
        // in the Batch API response, so this loop body never executed.
        // Real-world LFS endpoints (S3-signed URLs, Azure SAS, etc.)
        // include auth headers here that the client must forward.
        let mut server = mockito::Server::new();
        let oid = "abc123def4567890abc123def4567890abc123def4567890abc123def4567890";
        let payload = b"signed-url payload";
        let download_path = format!("/repo.git/info/lfs/objects/{oid}");
        let download_href = format!("{}{}", server.url(), download_path);

        // Batch response with a custom download header. The mock GET
        // below requires that header to be present on the request; if
        // the loop body didn't execute, the GET would fail to match
        // and the download would 501.
        let body = format!(
            r#"{{
                "objects": [
                    {{
                        "oid": "{oid}",
                        "size": {size},
                        "actions": {{
                            "download": {{
                                "href": "{download_href}",
                                "header": {{
                                    "x-amz-signature": "deadbeef"
                                }}
                            }}
                        }}
                    }}
                ]
            }}"#,
            size = payload.len(),
        );

        let _batch_mock = server
            .mock("POST", "/repo.git/info/lfs/objects/batch")
            .with_status(200)
            .with_header("content-type", "application/vnd.git-lfs+json")
            .with_body(body)
            .create();
        let _download_mock = server
            .mock("GET", download_path.as_str())
            .match_header("x-amz-signature", "deadbeef")
            .with_status(200)
            .with_body(payload)
            .create();

        let repo_url = format!("{}/repo.git", server.url());
        let config = fast_retry_config(3);
        let client = LfsClient::new(&repo_url, None, None, None, &config, None).unwrap();

        let pointer = make_pointer(oid, payload.len() as u64);
        let content = client.fetch_content(&pointer).unwrap();
        assert_eq!(content, payload);
    }

    #[test]
    fn fetch_content_does_not_retry_download_get_on_4xx() {
        // Lines 388-392: the non-retryable status return inside
        // download_chunked. The Batch API succeeds; the download GET
        // returns 404 (not in the transient-status set), so it must
        // surface the error without retrying.
        let mut server = mockito::Server::new();
        let oid = "abc123def4567890abc123def4567890abc123def4567890abc123def4567890";
        let download_path = format!("/repo.git/info/lfs/objects/{oid}");
        let download_href = format!("{}{}", server.url(), download_path);

        let _batch_mock = server
            .mock("POST", "/repo.git/info/lfs/objects/batch")
            .with_status(200)
            .with_header("content-type", "application/vnd.git-lfs+json")
            .with_body(make_batch_response(oid, 100, &download_href))
            .create();

        // 404 is non-retryable; mockito will fail the test if we hit
        // the GET more than once.
        let _download_mock = server
            .mock("GET", download_path.as_str())
            .with_status(404)
            .expect(1)
            .create();

        let repo_url = format!("{}/repo.git", server.url());
        let config = fast_retry_config(5);
        let client = LfsClient::new(&repo_url, None, None, None, &config, None).unwrap();

        let pointer = make_pointer(oid, 100);
        let result = client.fetch_content(&pointer);
        assert!(result.is_err(), "404 download must surface as an error");
        let msg = format!("{}", result.unwrap_err());
        assert!(msg.contains("404") || msg.contains("status"), "got: {msg}");
    }

    #[test]
    fn fetch_content_returns_connect_error_when_download_host_unreachable() {
        // Lines 308-310 / 394-410: `is_transient_error` and the
        // connection-error retry/return paths in download_chunked.
        // Point the download URL at a port nothing is listening on,
        // so reqwest produces a connect-error (which `is_transient_error`
        // returns true for, exercising the retry loop) and eventually
        // fails after exhausting attempts (the `return Err(...)` at
        // the bottom of the Err arm).
        let mut server = mockito::Server::new();
        let oid = "abc123def4567890abc123def4567890abc123def4567890abc123def4567890";
        // Port 1 is the system's TCPMUX port — almost never listening,
        // and the OS rejects with TCP RST immediately rather than
        // timing out. Avoids slow tests.
        let download_href = "http://127.0.0.1:1/unreachable";

        let _batch_mock = server
            .mock("POST", "/repo.git/info/lfs/objects/batch")
            .with_status(200)
            .with_header("content-type", "application/vnd.git-lfs+json")
            .with_body(make_batch_response(oid, 10, download_href))
            .create();

        let repo_url = format!("{}/repo.git", server.url());
        // Use a tight per-connect cap so the test stays fast even on
        // platforms where 127.0.0.1:1 doesn't RST immediately.
        let config = LfsConfig {
            retry_max_attempts: 2,
            retry_initial_backoff_ms: 1,
            retry_max_backoff_ms: 2,
            retry_backoff_multiplier: 2.0,
            max_object_size: None,
            request_timeout_secs: 5,
            connect_timeout_secs: 1,
            download_timeout_secs: 5,
        };
        let client = LfsClient::new(&repo_url, None, None, None, &config, None).unwrap();

        let pointer = make_pointer(oid, 10);
        let result = client.fetch_content(&pointer);
        assert!(
            result.is_err(),
            "unreachable download URL must surface as an error"
        );
        let msg = format!("{}", result.unwrap_err());
        // Either "LFS download failed" (connect-error path) or a
        // generic transport error — both prove the request reached
        // the network and failed gracefully rather than panicking.
        assert!(!msg.is_empty(), "error message must not be empty");
    }

    #[test]
    fn fetch_content_returns_connect_error_when_lfs_host_unreachable() {
        // Lines 550-566: the connection-error retry/return paths in
        // post_with_retry. Same trick as the download test above, but
        // pointed at the Batch API URL itself.
        let repo_url = "http://127.0.0.1:1/repo.git";
        let config = LfsConfig {
            retry_max_attempts: 2,
            retry_initial_backoff_ms: 1,
            retry_max_backoff_ms: 2,
            retry_backoff_multiplier: 2.0,
            max_object_size: None,
            request_timeout_secs: 5,
            connect_timeout_secs: 1,
            download_timeout_secs: 5,
        };
        let client = LfsClient::new(repo_url, None, None, None, &config, None).unwrap();

        let pointer = make_pointer("dead", 10);
        let result = client.fetch_content(&pointer);
        assert!(
            result.is_err(),
            "unreachable Batch API URL must surface as an error"
        );
        let msg = format!("{}", result.unwrap_err());
        // Should reference the batch-request failure, not download.
        assert!(
            msg.contains("LFS batch request failed") || msg.contains("batch"),
            "got: {msg}"
        );
    }

    #[test]
    fn fetch_content_warns_but_returns_when_actual_size_under_declared() {
        // Lines 689-695: the size-mismatch `warn!`. When the actual
        // download is smaller than `pointer.size`, we don't error —
        // we just log and return what we got. Without this test, that
        // branch is unreached (existing tests all have actual_len ==
        // pointer.size).
        //
        // The over-declared-size path is the safer of the two
        // mismatches: a too-small download just means truncation by
        // the server, not a security boundary failure. The actual
        // *over-size* path is the one with the security cap, which
        // `fetch_content_rejects_oversize_actual_response` covers.
        let mut server = mockito::Server::new();
        let oid = "abc123def4567890abc123def4567890abc123def4567890abc123def4567890";
        // Pointer claims 100 bytes; server actually sends 50.
        let actual_payload = vec![b'S'; 50];
        let claimed_size = 100u64;
        let download_path = format!("/repo.git/info/lfs/objects/{oid}");
        let download_href = format!("{}{}", server.url(), download_path);

        let _batch_mock = server
            .mock("POST", "/repo.git/info/lfs/objects/batch")
            .with_status(200)
            .with_header("content-type", "application/vnd.git-lfs+json")
            .with_body(make_batch_response(oid, claimed_size, &download_href))
            .create();
        let _download_mock = server
            .mock("GET", download_path.as_str())
            .with_status(200)
            .with_body(&actual_payload)
            .create();

        let repo_url = format!("{}/repo.git", server.url());
        let config = fast_retry_config(3);
        let client = LfsClient::new(&repo_url, None, None, None, &config, None).unwrap();

        let pointer = make_pointer(oid, claimed_size);
        let content = client.fetch_content(&pointer).unwrap();
        // Returned content is what the server actually sent, not the
        // claimed size.
        assert_eq!(content.len(), actual_payload.len());
        assert_eq!(content, actual_payload);
    }
}
