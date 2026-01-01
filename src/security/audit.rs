//! Audit logging for Git operations.
//!
//! This module provides comprehensive logging of all Git operations for
//! accountability and security monitoring.
//!
//! # Log Format
//!
//! Each log entry is a JSON object with:
//! - `timestamp`: ISO 8601 timestamp
//! - `event_type`: Type of event (`command_executed`, `command_blocked`, etc.)
//! - `command`: The Git command that was executed
//! - `args`: Command arguments (sanitised)
//! - `working_dir`: Working directory (if set)
//! - `outcome`: Success, failure, or blocked
//! - `reason`: Reason for blocking (if blocked)
//! - `duration_ms`: Execution time in milliseconds (if executed)

use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use serde::Serialize;

/// Outcome of a Git operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditOutcome {
    /// Command executed successfully.
    Success,

    /// Command executed but failed.
    Failed,

    /// Command was blocked by security policy.
    Blocked,
}

/// Type of audit event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditEventType {
    /// A Git command was executed.
    CommandExecuted,

    /// A Git command was blocked.
    CommandBlocked,

    /// Rate limit was exceeded.
    RateLimitExceeded,

    /// Server started.
    ServerStarted,

    /// Server stopped.
    ServerStopped,
}

/// Reason for server shutdown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ShutdownReason {
    /// Client closed the connection (stdin closed).
    ClientDisconnected,

    /// Received SIGINT signal (Ctrl+C).
    SigInt,

    /// Received SIGTERM signal.
    SigTerm,
}

/// An audit event to be logged.
#[derive(Debug, Clone, Serialize)]
pub struct AuditEvent {
    /// ISO 8601 timestamp.
    pub timestamp: String,

    /// Type of event.
    pub event_type: AuditEventType,

    /// The Git command.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,

    /// Command arguments (sanitised).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,

    /// Working directory.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working_dir: Option<PathBuf>,

    /// Outcome of the operation.
    pub outcome: AuditOutcome,

    /// Reason for blocking (if blocked).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,

    /// Execution duration in milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,

    /// Exit code (if executed).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,

    /// Reason for server shutdown (if server stopped event).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shutdown_reason: Option<ShutdownReason>,
}

impl AuditEvent {
    /// Creates a new audit event with the current timestamp.
    #[must_use]
    pub fn new(event_type: AuditEventType, outcome: AuditOutcome) -> Self {
        Self {
            timestamp: Self::current_timestamp(),
            event_type,
            command: None,
            args: None,
            working_dir: None,
            outcome,
            reason: None,
            duration_ms: None,
            exit_code: None,
            shutdown_reason: None,
        }
    }

    /// Creates an event for a successfully executed command.
    #[must_use]
    pub fn command_success(
        command: impl Into<String>,
        args: Vec<String>,
        working_dir: Option<PathBuf>,
        duration: Duration,
        exit_code: i32,
    ) -> Self {
        let outcome = if exit_code == 0 {
            AuditOutcome::Success
        } else {
            AuditOutcome::Failed
        };

        Self {
            timestamp: Self::current_timestamp(),
            event_type: AuditEventType::CommandExecuted,
            command: Some(command.into()),
            args: Some(args),
            working_dir,
            outcome,
            reason: None,
            #[allow(clippy::cast_possible_truncation)] // Duration in ms fits in u64
            duration_ms: Some(duration.as_millis() as u64),
            exit_code: Some(exit_code),
            shutdown_reason: None,
        }
    }

    /// Creates an event for a blocked command.
    #[must_use]
    pub fn command_blocked(
        command: impl Into<String>,
        args: Vec<String>,
        working_dir: Option<PathBuf>,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            timestamp: Self::current_timestamp(),
            event_type: AuditEventType::CommandBlocked,
            command: Some(command.into()),
            args: Some(args),
            working_dir,
            outcome: AuditOutcome::Blocked,
            reason: Some(reason.into()),
            duration_ms: None,
            exit_code: None,
            shutdown_reason: None,
        }
    }

    /// Creates an event for rate limit exceeded.
    #[must_use]
    pub fn rate_limit_exceeded(
        command: impl Into<String>,
        args: Vec<String>,
        working_dir: Option<PathBuf>,
    ) -> Self {
        Self {
            timestamp: Self::current_timestamp(),
            event_type: AuditEventType::RateLimitExceeded,
            command: Some(command.into()),
            args: Some(args),
            working_dir,
            outcome: AuditOutcome::Blocked,
            reason: Some("Rate limit exceeded".to_string()),
            duration_ms: None,
            exit_code: None,
            shutdown_reason: None,
        }
    }

    /// Creates an event for server start.
    #[must_use]
    pub fn server_started() -> Self {
        Self::new(AuditEventType::ServerStarted, AuditOutcome::Success)
    }

    /// Creates an event for server stop with the shutdown reason.
    #[must_use]
    pub fn server_stopped(reason: ShutdownReason) -> Self {
        let mut event = Self::new(AuditEventType::ServerStopped, AuditOutcome::Success);
        event.shutdown_reason = Some(reason);
        event
    }

    /// Gets the current timestamp in ISO 8601 format.
    fn current_timestamp() -> String {
        let now = SystemTime::now();
        let duration = now
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default();

        // Format as ISO 8601 without external dependencies
        let secs = duration.as_secs();
        let millis = duration.subsec_millis();

        // Calculate date/time components
        let days_since_epoch = secs / 86400;
        let time_of_day = secs % 86400;
        let hours = time_of_day / 3600;
        let minutes = (time_of_day % 3600) / 60;
        let seconds = time_of_day % 60;

        // Simplified date calculation (not accounting for leap years perfectly)
        let (year, month, day) = days_to_ymd(days_since_epoch);

        format!("{year:04}-{month:02}-{day:02}T{hours:02}:{minutes:02}:{seconds:02}.{millis:03}Z")
    }
}

/// Converts days since Unix epoch to year/month/day.
#[allow(clippy::cast_possible_wrap)] // Days since 1970 won't overflow i64
#[allow(clippy::cast_possible_truncation)] // Day of month fits in u32
#[allow(clippy::cast_sign_loss)] // Day is always positive after calculation
fn days_to_ymd(days: u64) -> (u32, u32, u32) {
    // Simplified calculation - good enough for logging purposes
    let mut remaining_days = days as i64;
    let mut year = 1970;

    loop {
        let days_in_year = if is_leap_year(year) { 366 } else { 365 };
        if remaining_days < days_in_year {
            break;
        }
        remaining_days -= days_in_year;
        year += 1;
    }

    let days_in_months: [i64; 12] = if is_leap_year(year) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };

    let mut month = 1;
    for days_in_month in days_in_months {
        if remaining_days < days_in_month {
            break;
        }
        remaining_days -= days_in_month;
        month += 1;
    }

    let day = remaining_days as u32 + 1;
    (year, month, day)
}

/// Checks if a year is a leap year.
const fn is_leap_year(year: u32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

/// Audit logger that writes events to a file.
pub struct AuditLogger {
    /// Path to the log file.
    log_path: PathBuf,

    /// Buffered writer (wrapped in Mutex for thread safety).
    writer: Mutex<Option<BufWriter<File>>>,

    /// Whether logging is enabled.
    enabled: bool,
}

impl AuditLogger {
    /// Creates a new audit logger.
    ///
    /// # Arguments
    ///
    /// * `log_path` — Path to the log file
    ///
    /// # Errors
    ///
    /// Returns an error if the log file cannot be created or opened.
    pub fn new(log_path: impl AsRef<Path>) -> Result<Self, AuditLoggerError> {
        let log_path = log_path.as_ref().to_path_buf();

        // Create parent directories if needed
        if let Some(parent) = log_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| AuditLoggerError::IoError {
                message: format!("Failed to create log directory: {e}"),
            })?;
        }

        // Open the file in append mode
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .map_err(|e| AuditLoggerError::IoError {
                message: format!("Failed to open log file: {e}"),
            })?;

        let writer = BufWriter::new(file);

        Ok(Self {
            log_path,
            writer: Mutex::new(Some(writer)),
            enabled: true,
        })
    }

    /// Creates a disabled audit logger (no-op).
    #[must_use]
    pub fn disabled() -> Self {
        Self {
            log_path: PathBuf::new(),
            writer: Mutex::new(None),
            enabled: false,
        }
    }

    /// Returns whether logging is enabled.
    #[must_use]
    pub const fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Returns the log file path.
    #[must_use]
    pub fn log_path(&self) -> &Path {
        &self.log_path
    }

    /// Logs an audit event.
    ///
    /// # Errors
    ///
    /// Returns an error if writing to the log file fails.
    #[allow(clippy::significant_drop_tightening)] // Lock must be held while writing
    pub fn log(&self, event: &AuditEvent) -> Result<(), AuditLoggerError> {
        if !self.enabled {
            return Ok(());
        }

        let json =
            serde_json::to_string(event).map_err(|e| AuditLoggerError::SerializationError {
                message: e.to_string(),
            })?;

        let mut guard = self
            .writer
            .lock()
            .map_err(|_| AuditLoggerError::LockError)?;

        if let Some(writer) = guard.as_mut() {
            writeln!(writer, "{json}").map_err(|e| AuditLoggerError::IoError {
                message: format!("Failed to write log entry: {e}"),
            })?;

            writer.flush().map_err(|e| AuditLoggerError::IoError {
                message: format!("Failed to flush log: {e}"),
            })?;
        }

        Ok(())
    }

    /// Logs an event, ignoring any errors.
    ///
    /// Use this for non-critical logging where failures shouldn't affect
    /// the main operation.
    pub fn log_silent(&self, event: &AuditEvent) {
        let _ = self.log(event);
    }
}

/// Errors that can occur during audit logging.
#[derive(Debug, thiserror::Error)]
pub enum AuditLoggerError {
    /// I/O error.
    #[error("I/O error: {message}")]
    IoError {
        /// Error message.
        message: String,
    },

    /// Serialization error.
    #[error("serialization error: {message}")]
    SerializationError {
        /// Error message.
        message: String,
    },

    /// Failed to acquire lock.
    #[error("failed to acquire lock on audit logger")]
    LockError,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use tempfile::NamedTempFile;

    #[test]
    fn audit_event_command_success() {
        let event = AuditEvent::command_success(
            "clone",
            vec!["https://github.com/user/repo.git".to_string()],
            None,
            Duration::from_millis(1234),
            0,
        );

        assert_eq!(event.event_type, AuditEventType::CommandExecuted);
        assert_eq!(event.outcome, AuditOutcome::Success);
        assert_eq!(event.command, Some("clone".to_string()));
        assert_eq!(event.duration_ms, Some(1234));
        assert_eq!(event.exit_code, Some(0));
    }

    #[test]
    fn audit_event_command_failed() {
        let event = AuditEvent::command_success(
            "push",
            vec!["origin".to_string(), "main".to_string()],
            None,
            Duration::from_millis(500),
            1,
        );

        assert_eq!(event.outcome, AuditOutcome::Failed);
        assert_eq!(event.exit_code, Some(1));
    }

    #[test]
    fn audit_event_command_blocked() {
        let event = AuditEvent::command_blocked(
            "push",
            vec!["--force".to_string()],
            None,
            "Force push is not allowed",
        );

        assert_eq!(event.event_type, AuditEventType::CommandBlocked);
        assert_eq!(event.outcome, AuditOutcome::Blocked);
        assert_eq!(event.reason, Some("Force push is not allowed".to_string()));
    }

    #[test]
    fn audit_event_rate_limit() {
        let event = AuditEvent::rate_limit_exceeded("clone", vec![], None);

        assert_eq!(event.event_type, AuditEventType::RateLimitExceeded);
        assert_eq!(event.outcome, AuditOutcome::Blocked);
    }

    #[test]
    fn audit_event_serialization() {
        let event = AuditEvent::command_success("status", vec![], None, Duration::from_secs(1), 0);

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"event_type\":\"command_executed\""));
        assert!(json.contains("\"outcome\":\"success\""));
    }

    #[test]
    fn audit_logger_writes_to_file() {
        let temp_file = NamedTempFile::new().unwrap();
        let path = temp_file.path().to_path_buf();

        let logger = AuditLogger::new(&path).unwrap();

        let event = AuditEvent::server_started();
        logger.log(&event).unwrap();

        // Read back the file
        drop(logger); // Ensure file is flushed
        let mut file = File::open(&path).unwrap();
        let mut contents = String::new();
        file.read_to_string(&mut contents).unwrap();

        assert!(contents.contains("\"event_type\":\"server_started\""));
    }

    #[test]
    fn audit_logger_disabled() {
        let logger = AuditLogger::disabled();

        assert!(!logger.is_enabled());

        // Should not error even when disabled
        let event = AuditEvent::server_started();
        logger.log(&event).unwrap();
    }

    #[test]
    fn timestamp_format() {
        let event = AuditEvent::server_started();

        // Should be in ISO 8601 format
        assert!(event.timestamp.contains('T'));
        assert!(event.timestamp.ends_with('Z'));
        assert_eq!(event.timestamp.len(), 24); // "2024-01-15T12:30:45.123Z"
    }

    #[test]
    fn leap_year_detection() {
        assert!(is_leap_year(2000)); // Divisible by 400
        assert!(!is_leap_year(1900)); // Divisible by 100 but not 400
        assert!(is_leap_year(2024)); // Divisible by 4 but not 100
        assert!(!is_leap_year(2023)); // Not divisible by 4
    }

    #[test]
    fn audit_event_server_stopped_with_reason() {
        let event = AuditEvent::server_stopped(ShutdownReason::SigInt);

        assert_eq!(event.event_type, AuditEventType::ServerStopped);
        assert_eq!(event.outcome, AuditOutcome::Success);
        assert_eq!(event.shutdown_reason, Some(ShutdownReason::SigInt));
    }

    #[test]
    fn shutdown_reason_serialisation() {
        let event = AuditEvent::server_stopped(ShutdownReason::ClientDisconnected);
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"shutdown_reason\":\"client_disconnected\""));

        let event = AuditEvent::server_stopped(ShutdownReason::SigTerm);
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"shutdown_reason\":\"sig_term\""));

        let event = AuditEvent::server_stopped(ShutdownReason::SigInt);
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"shutdown_reason\":\"sig_int\""));
    }
}
