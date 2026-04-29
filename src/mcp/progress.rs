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

    #[test]
    fn lfs_download_percentage() {
        let update = ProgressUpdate::LfsDownload {
            downloaded: 3,
            total: 10,
            current_file: None,
            bytes_downloaded: 0,
            bytes_total: 0,
        };
        assert_eq!(update.percentage(), Some(30));
    }

    #[test]
    fn lfs_download_percentage_zero_total() {
        let update = ProgressUpdate::LfsDownload {
            downloaded: 0,
            total: 0,
            current_file: None,
            bytes_downloaded: 0,
            bytes_total: 0,
        };
        assert_eq!(update.percentage(), None);
    }

    #[test]
    fn submodule_fetch_percentage() {
        let update = ProgressUpdate::SubmoduleFetch {
            fetched: 2,
            total: 5,
            current_path: Some("vendor/x".to_string()),
        };
        assert_eq!(update.percentage(), Some(40));
    }

    #[test]
    fn submodule_fetch_percentage_zero_total() {
        let update = ProgressUpdate::SubmoduleFetch {
            fetched: 0,
            total: 0,
            current_path: None,
        };
        assert_eq!(update.percentage(), None);
    }

    #[test]
    fn message_progress_percentage() {
        let update = ProgressUpdate::Message {
            progress: 73,
            message: "working".to_string(),
        };
        assert_eq!(update.percentage(), Some(73));
    }

    #[test]
    fn file_processing_percentage_zero_total() {
        let update = ProgressUpdate::FileProcessing {
            processed: 0,
            total: 0,
            current_file: None,
        };
        assert_eq!(update.percentage(), None);
    }

    #[test]
    fn transfer_description_no_total_bytes() {
        let update = ProgressUpdate::Transfer {
            received_bytes: 100,
            total_bytes: 0,
            received_objects: 5,
            total_objects: 10,
            indexed_objects: 5,
        };
        let desc = update.description();
        assert!(desc.contains("5/10 objects"));
        assert!(desc.contains("100 B"));
    }

    #[test]
    fn file_processing_description_with_current() {
        let update = ProgressUpdate::FileProcessing {
            processed: 5,
            total: 10,
            current_file: Some("src/lib.rs".to_string()),
        };
        let desc = update.description();
        assert!(desc.contains("5/10"));
        assert!(desc.contains("src/lib.rs"));
    }

    #[test]
    fn file_processing_description_zero_total() {
        let update = ProgressUpdate::FileProcessing {
            processed: 5,
            total: 0,
            current_file: None,
        };
        let desc = update.description();
        assert!(desc.contains("Processing files: 5"));
        assert!(!desc.contains('/'));
    }

    #[test]
    fn lfs_download_description() {
        let update = ProgressUpdate::LfsDownload {
            downloaded: 2,
            total: 5,
            current_file: Some("video.mp4".to_string()),
            bytes_downloaded: 524_288,
            bytes_total: 1_048_576,
        };
        let desc = update.description();
        assert!(desc.contains("2/5"));
        assert!(desc.contains("video.mp4"));
        assert!(desc.contains("KB"));
    }

    #[test]
    fn lfs_download_description_no_bytes() {
        let update = ProgressUpdate::LfsDownload {
            downloaded: 1,
            total: 3,
            current_file: None,
            bytes_downloaded: 0,
            bytes_total: 0,
        };
        let desc = update.description();
        assert!(desc.contains("1/3"));
    }

    #[test]
    fn submodule_fetch_description_with_path() {
        let update = ProgressUpdate::SubmoduleFetch {
            fetched: 1,
            total: 3,
            current_path: Some("vendor/lib".to_string()),
        };
        let desc = update.description();
        assert!(desc.contains("1/3"));
        assert!(desc.contains("vendor/lib"));
    }

    #[test]
    fn submodule_fetch_description_without_path() {
        let update = ProgressUpdate::SubmoduleFetch {
            fetched: 0,
            total: 2,
            current_path: None,
        };
        let desc = update.description();
        assert!(desc.contains("0/2"));
    }

    #[test]
    fn message_description_returns_message() {
        let update = ProgressUpdate::Message {
            progress: 50,
            message: "halfway done".to_string(),
        };
        assert_eq!(update.description(), "halfway done");
    }

    #[test]
    fn complete_description_includes_duration() {
        let update = ProgressUpdate::Complete {
            duration: Duration::from_secs_f64(3.5),
        };
        let desc = update.description();
        assert!(desc.contains("3.5"));
        assert!(desc.contains("Complete"));
    }

    #[test]
    fn format_bytes_just_below_kb() {
        assert_eq!(format_bytes(1023), "1023 B");
    }

    #[test]
    fn format_bytes_just_below_mb() {
        let result = format_bytes(1024 * 1024 - 1);
        assert!(result.contains("KB"));
    }

    #[test]
    fn format_bytes_just_below_gb() {
        let result = format_bytes(1024 * 1024 * 1024 - 1);
        assert!(result.contains("MB"));
    }

    #[test]
    fn format_bytes_zero() {
        assert_eq!(format_bytes(0), "0 B");
    }

    #[test]
    fn format_bytes_large_gb() {
        let result = format_bytes(5 * 1024 * 1024 * 1024);
        assert!(result.contains("GB"));
        assert!(result.contains("5.0"));
    }

    #[test]
    fn progress_sender_send_transfer() {
        let (tx, rx) = ProgressSender::new("t".to_string());
        tx.send_transfer(100, 200, 5, 10, 5);
        let received = rx.recv().unwrap();
        match received {
            ProgressUpdate::Transfer {
                received_bytes,
                total_bytes,
                received_objects,
                total_objects,
                indexed_objects,
            } => {
                assert_eq!(received_bytes, 100);
                assert_eq!(total_bytes, 200);
                assert_eq!(received_objects, 5);
                assert_eq!(total_objects, 10);
                assert_eq!(indexed_objects, 5);
            }
            other => panic!("expected Transfer, got {other:?}"),
        }
    }

    #[test]
    fn progress_sender_send_file_progress() {
        let (tx, rx) = ProgressSender::new("t".to_string());
        tx.send_file_progress(1, 5, Some("a.txt"));
        let received = rx.recv().unwrap();
        match received {
            ProgressUpdate::FileProcessing {
                processed,
                total,
                current_file,
            } => {
                assert_eq!(processed, 1);
                assert_eq!(total, 5);
                assert_eq!(current_file, Some("a.txt".to_string()));
            }
            other => panic!("expected FileProcessing, got {other:?}"),
        }
    }

    #[test]
    fn progress_sender_send_lfs_progress() {
        let (tx, rx) = ProgressSender::new("t".to_string());
        tx.send_lfs_progress(1, 3, Some("file.bin"), 100, 200);
        let received = rx.recv().unwrap();
        assert!(matches!(received, ProgressUpdate::LfsDownload { .. }));
    }

    #[test]
    fn progress_sender_send_submodule_progress() {
        let (tx, rx) = ProgressSender::new("t".to_string());
        tx.send_submodule_progress(2, 4, Some("vendor/x"));
        let received = rx.recv().unwrap();
        assert!(matches!(received, ProgressUpdate::SubmoduleFetch { .. }));
    }

    #[test]
    fn progress_sender_send_complete() {
        let (tx, rx) = ProgressSender::new("t".to_string());
        tx.send_complete(Duration::from_secs(2));
        let received = rx.recv().unwrap();
        assert!(matches!(received, ProgressUpdate::Complete { .. }));
    }

    #[test]
    fn progress_sender_can_be_cloned() {
        let (tx, _rx) = ProgressSender::new("t".to_string());
        let cloned = tx.clone();
        assert_eq!(cloned.token(), tx.token());
    }

    #[test]
    fn progress_sender_send_after_receiver_dropped_returns_false() {
        let (tx, rx) = ProgressSender::new("t".to_string());
        drop(rx);
        // Even though it's a Complete (no rate limit), channel is closed
        assert!(!tx.send(ProgressUpdate::Complete {
            duration: Duration::from_secs(1),
        }));
    }

    #[test]
    fn progress_notification_from_update_includes_message() {
        let update = ProgressUpdate::Message {
            progress: 42,
            message: "test message".to_string(),
        };
        let notif = ProgressNotification::from_update("tok", &update);
        assert_eq!(notif.progress_token, "tok");
        assert_eq!(notif.progress, 42);
        assert_eq!(notif.total, Some(100));
        assert_eq!(notif.message.as_deref(), Some("test message"));
    }

    #[test]
    fn progress_notification_from_unknown_total_uses_zero() {
        let update = ProgressUpdate::FileProcessing {
            processed: 5,
            total: 0,
            current_file: None,
        };
        let notif = ProgressNotification::from_update("tok", &update);
        assert_eq!(notif.progress, 0);
    }

    #[test]
    fn create_progress_notification_has_jsonrpc_envelope() {
        let update = ProgressUpdate::Complete {
            duration: Duration::from_secs(1),
        };
        let json = create_progress_notification("tok", &update);
        assert_eq!(json["jsonrpc"], "2.0");
        assert_eq!(json["method"], "notifications/progress");
        assert!(json["params"].is_object());
    }
}
