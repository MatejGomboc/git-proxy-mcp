//! Progress notification support for MCP tools.
//!
//! This module provides real-time progress updates during long-running operations
//! like repository cloning, LFS downloads, and submodule fetching.
//!
//! # MCP Progress Protocol
//!
//! MCP supports progress notifications for long-running requests. The client
//! includes a `_meta.progressToken` in the request, and the server sends
//! `notifications/progress` messages with that token.
//!
//! # Architecture
//!
//! Since git2 operations are synchronous but the transport is async, we use
//! a channel to bridge the two:
//!
//! ```text
//! [Sync git2 callback] -> channel -> [Async transport writer]
//! ```
//!
//! The `ProgressSender` is passed to sync operations and sends updates through
//! the channel. An async task receives and forwards to the MCP client.

use serde::Serialize;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Minimum interval between progress notifications to avoid flooding.
const MIN_PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

/// A progress update from an operation.
#[derive(Debug, Clone)]
pub enum ProgressUpdate {
    /// Network transfer progress (fetching git objects).
    Transfer {
        /// Bytes received so far.
        received_bytes: usize,
        /// Total bytes expected (0 if unknown).
        total_bytes: usize,
        /// Objects received.
        received_objects: usize,
        /// Total objects expected.
        total_objects: usize,
        /// Objects indexed locally.
        indexed_objects: usize,
    },

    /// File processing progress (building tar archive).
    FileProcessing {
        /// Files processed so far.
        processed: usize,
        /// Total files to process (0 if unknown).
        total: usize,
        /// Current file being processed.
        current_file: Option<String>,
    },

    /// LFS download progress.
    LfsDownload {
        /// Files downloaded so far.
        downloaded: usize,
        /// Total files to download.
        total: usize,
        /// Current file being downloaded.
        current_file: Option<String>,
        /// Bytes downloaded for current file.
        bytes_downloaded: usize,
        /// Total bytes for current file.
        bytes_total: usize,
    },

    /// Submodule fetch progress.
    SubmoduleFetch {
        /// Submodules fetched so far.
        fetched: usize,
        /// Total submodules to fetch.
        total: usize,
        /// Current submodule path.
        current_path: Option<String>,
    },

    /// Generic progress with a message.
    Message {
        /// Progress percentage (0-100).
        progress: u32,
        /// Human-readable message.
        message: String,
    },

    /// Operation completed.
    Complete {
        /// Total duration of the operation.
        duration: Duration,
    },
}

impl ProgressUpdate {
    /// Returns the progress percentage (0-100), or None if unknown.
    ///
    /// The percentage is calculated as `(current / total) * 100`.
    /// Returns `None` if total is 0 or unknown.
    #[must_use]
    #[allow(clippy::cast_possible_truncation)] // Percentage is always 0-100
    #[allow(clippy::missing_const_for_fn)] // match with non-const patterns
    pub fn percentage(&self) -> Option<u32> {
        match self {
            Self::Transfer {
                received_objects,
                total_objects,
                ..
            } => {
                if *total_objects > 0 {
                    Some(((*received_objects as u64 * 100) / (*total_objects as u64)) as u32)
                } else {
                    None
                }
            }
            Self::FileProcessing {
                processed, total, ..
            } => {
                if *total > 0 {
                    Some(((*processed as u64 * 100) / (*total as u64)) as u32)
                } else {
                    None
                }
            }
            Self::LfsDownload {
                downloaded, total, ..
            } => {
                if *total > 0 {
                    Some(((*downloaded as u64 * 100) / (*total as u64)) as u32)
                } else {
                    None
                }
            }
            Self::SubmoduleFetch { fetched, total, .. } => {
                if *total > 0 {
                    Some(((*fetched as u64 * 100) / (*total as u64)) as u32)
                } else {
                    None
                }
            }
            Self::Message { progress, .. } => Some(*progress),
            Self::Complete { .. } => Some(100),
        }
    }

    /// Returns a human-readable description of the progress.
    #[must_use]
    pub fn description(&self) -> String {
        match self {
            Self::Transfer {
                received_bytes,
                total_bytes,
                received_objects,
                total_objects,
                ..
            } => {
                let bytes_str = if *total_bytes > 0 {
                    format!(
                        "{} / {} bytes",
                        format_bytes(*received_bytes),
                        format_bytes(*total_bytes)
                    )
                } else {
                    format!("{} bytes", format_bytes(*received_bytes))
                };
                format!("Fetching: {received_objects}/{total_objects} objects ({bytes_str})")
            }
            Self::FileProcessing {
                processed,
                total,
                current_file,
            } => {
                let file_info = current_file
                    .as_ref()
                    .map(|f| format!(": {f}"))
                    .unwrap_or_default();
                if *total > 0 {
                    format!("Processing files: {processed}/{total}{file_info}")
                } else {
                    format!("Processing files: {processed}{file_info}")
                }
            }
            Self::LfsDownload {
                downloaded,
                total,
                current_file,
                bytes_downloaded,
                bytes_total,
            } => {
                let file_info = current_file
                    .as_ref()
                    .map(|f| format!(": {f}"))
                    .unwrap_or_default();
                let bytes_str = if *bytes_total > 0 {
                    format!(
                        " ({}/{})",
                        format_bytes(*bytes_downloaded),
                        format_bytes(*bytes_total)
                    )
                } else {
                    String::new()
                };
                format!("Downloading LFS: {downloaded}/{total}{file_info}{bytes_str}")
            }
            Self::SubmoduleFetch {
                fetched,
                total,
                current_path,
            } => {
                let path_info = current_path
                    .as_ref()
                    .map(|p| format!(": {p}"))
                    .unwrap_or_default();
                format!("Fetching submodules: {fetched}/{total}{path_info}")
            }
            Self::Message { message, .. } => message.clone(),
            Self::Complete { duration } => {
                format!("Complete in {:.1}s", duration.as_secs_f64())
            }
        }
    }
}

/// Formats bytes in human-readable form.
#[allow(clippy::cast_precision_loss)] // Precision loss is acceptable for display
fn format_bytes(bytes: usize) -> String {
    const KB: usize = 1024;
    const MB: usize = 1024 * KB;
    const GB: usize = 1024 * MB;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}

/// Sender for progress updates from sync code.
///
/// This is cheap to clone and can be passed to multiple callbacks.
#[derive(Debug, Clone)]
pub struct ProgressSender {
    /// The channel sender.
    sender: mpsc::Sender<ProgressUpdate>,
    /// Progress token from the client request.
    token: Arc<String>,
    /// Last time we sent a progress update (for rate limiting).
    last_sent: Arc<std::sync::Mutex<Instant>>,
}

impl ProgressSender {
    /// Creates a new progress sender with the given token.
    #[must_use]
    pub fn new(token: String) -> (Self, mpsc::Receiver<ProgressUpdate>) {
        let (sender, receiver) = mpsc::channel();
        // Initialize last_sent to a time in the past so first send always succeeds.
        // Use 2x interval to ensure first update passes even with timing jitter.
        // If subtraction fails (theoretically on freshly booted system), use now
        // which may rate-limit the first update - acceptable edge case.
        let initial_time = Instant::now()
            .checked_sub(MIN_PROGRESS_INTERVAL * 2)
            .unwrap_or_else(Instant::now);
        (
            Self {
                sender,
                token: Arc::new(token),
                last_sent: Arc::new(std::sync::Mutex::new(initial_time)),
            },
            receiver,
        )
    }

    /// Returns the progress token.
    #[must_use]
    pub fn token(&self) -> &str {
        &self.token
    }

    /// Sends a progress update, respecting rate limiting.
    ///
    /// Returns true if the update was sent, false if rate-limited or channel closed.
    ///
    /// # Mutex Poisoning
    ///
    /// If the internal mutex is poisoned (a thread panicked while holding the lock),
    /// this method recovers gracefully by extracting the inner value and continuing.
    /// A warning is logged when this occurs.
    #[must_use]
    pub fn send(&self, update: ProgressUpdate) -> bool {
        // Always send Complete updates
        let is_complete = matches!(update, ProgressUpdate::Complete { .. });

        if !is_complete {
            // Rate limit non-complete updates
            let mut last_sent = self.last_sent.lock().unwrap_or_else(|poisoned| {
                tracing::warn!(
                    "Progress sender mutex was poisoned; recovering with potentially stale state"
                );
                poisoned.into_inner()
            });
            if last_sent.elapsed() < MIN_PROGRESS_INTERVAL {
                return false;
            }
            *last_sent = Instant::now();
        }

        self.sender.send(update).is_ok()
    }

    /// Sends a transfer progress update.
    ///
    /// This is a fire-and-forget convenience method. The return value of `send()`
    /// is intentionally ignored since callers typically don't need to know if
    /// the update was rate-limited.
    pub fn send_transfer(
        &self,
        received_bytes: usize,
        total_bytes: usize,
        received_objects: usize,
        total_objects: usize,
        indexed_objects: usize,
    ) {
        let _ = self.send(ProgressUpdate::Transfer {
            received_bytes,
            total_bytes,
            received_objects,
            total_objects,
            indexed_objects,
        });
    }

    /// Sends a file processing progress update.
    ///
    /// This is a fire-and-forget convenience method.
    pub fn send_file_progress(&self, processed: usize, total: usize, current_file: Option<&str>) {
        let _ = self.send(ProgressUpdate::FileProcessing {
            processed,
            total,
            current_file: current_file.map(String::from),
        });
    }

    /// Sends an LFS download progress update.
    ///
    /// This is a fire-and-forget convenience method.
    pub fn send_lfs_progress(
        &self,
        downloaded: usize,
        total: usize,
        current_file: Option<&str>,
        bytes_downloaded: usize,
        bytes_total: usize,
    ) {
        let _ = self.send(ProgressUpdate::LfsDownload {
            downloaded,
            total,
            current_file: current_file.map(String::from),
            bytes_downloaded,
            bytes_total,
        });
    }

    /// Sends a submodule fetch progress update.
    ///
    /// This is a fire-and-forget convenience method.
    pub fn send_submodule_progress(
        &self,
        fetched: usize,
        total: usize,
        current_path: Option<&str>,
    ) {
        let _ = self.send(ProgressUpdate::SubmoduleFetch {
            fetched,
            total,
            current_path: current_path.map(String::from),
        });
    }

    /// Sends a completion update.
    ///
    /// This is a fire-and-forget convenience method.
    pub fn send_complete(&self, duration: Duration) {
        let _ = self.send(ProgressUpdate::Complete { duration });
    }
}

/// MCP progress notification payload.
///
/// Sent as `notifications/progress` to the client.
#[derive(Debug, Clone, Serialize)]
pub struct ProgressNotification {
    /// The progress token from the request.
    #[serde(rename = "progressToken")]
    pub progress_token: String,

    /// Progress value (0-100 or custom scale).
    pub progress: u32,

    /// Optional total for the progress scale.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<u32>,

    /// Optional human-readable message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl ProgressNotification {
    /// Creates a progress notification from an update.
    #[must_use]
    pub fn from_update(token: &str, update: &ProgressUpdate) -> Self {
        Self {
            progress_token: token.to_string(),
            progress: update.percentage().unwrap_or(0),
            total: Some(100),
            message: Some(update.description()),
        }
    }
}

/// Creates an MCP notification JSON value for progress.
#[must_use]
pub fn create_progress_notification(token: &str, update: &ProgressUpdate) -> serde_json::Value {
    let notification = ProgressNotification::from_update(token, update);
    serde_json::json!({
        "jsonrpc": "2.0",
        "method": "notifications/progress",
        "params": notification
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transfer_progress_percentage() {
        let update = ProgressUpdate::Transfer {
            received_bytes: 500,
            total_bytes: 1000,
            received_objects: 50,
            total_objects: 100,
            indexed_objects: 45,
        };
        assert_eq!(update.percentage(), Some(50));
    }

    #[test]
    fn file_processing_percentage() {
        let update = ProgressUpdate::FileProcessing {
            processed: 75,
            total: 100,
            current_file: Some("src/main.rs".to_string()),
        };
        assert_eq!(update.percentage(), Some(75));
    }

    #[test]
    fn unknown_total_returns_none() {
        let update = ProgressUpdate::Transfer {
            received_bytes: 500,
            total_bytes: 0,
            received_objects: 50,
            total_objects: 0,
            indexed_objects: 45,
        };
        assert_eq!(update.percentage(), None);
    }

    #[test]
    fn complete_always_100() {
        let update = ProgressUpdate::Complete {
            duration: Duration::from_secs(5),
        };
        assert_eq!(update.percentage(), Some(100));
    }

    #[test]
    fn format_bytes_units() {
        assert_eq!(format_bytes(500), "500 B");
        assert_eq!(format_bytes(1024), "1.0 KB");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(1_048_576), "1.0 MB");
        assert_eq!(format_bytes(1_073_741_824), "1.0 GB");
    }

    #[test]
    fn transfer_description() {
        let update = ProgressUpdate::Transfer {
            received_bytes: 1_048_576,
            total_bytes: 2_097_152,
            received_objects: 50,
            total_objects: 100,
            indexed_objects: 45,
        };
        let desc = update.description();
        assert!(desc.contains("50/100 objects"));
        assert!(desc.contains("1.0 MB"));
    }

    #[test]
    fn progress_sender_rate_limiting() {
        let (sender, _receiver) = ProgressSender::new("test-token".to_string());

        // First send should succeed
        assert!(sender.send(ProgressUpdate::Message {
            progress: 10,
            message: "test".to_string(),
        }));

        // Immediate second send should be rate-limited
        assert!(!sender.send(ProgressUpdate::Message {
            progress: 20,
            message: "test".to_string(),
        }));

        // Complete should always send
        assert!(sender.send(ProgressUpdate::Complete {
            duration: Duration::from_secs(1),
        }));
    }

    #[test]
    fn progress_notification_serialization() {
        let update = ProgressUpdate::FileProcessing {
            processed: 50,
            total: 100,
            current_file: Some("test.rs".to_string()),
        };

        let notification = create_progress_notification("token-123", &update);
        let json = serde_json::to_string(&notification).unwrap();

        assert!(json.contains("notifications/progress"));
        assert!(json.contains("token-123"));
        assert!(json.contains("50"));
    }

    #[test]
    fn progress_token_preserved() {
        let (sender, _receiver) = ProgressSender::new("my-progress-token".to_string());
        assert_eq!(sender.token(), "my-progress-token");
    }
}
