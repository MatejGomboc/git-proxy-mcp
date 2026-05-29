//! stdio transport for MCP server.
//!
//! This module implements the stdio transport as specified by MCP:
//!
//! - Messages are UTF-8 encoded JSON-RPC
//! - Messages are delimited by newlines
//! - Messages must not contain embedded newlines
//! - stdin: receives messages from client
//! - stdout: sends messages to client
//! - stderr: may be used for logging (not MCP messages)
//!
//! # Concurrency model
//!
//! The transport uses async I/O with Tokio but is driven from a single task:
//! the server owns one [`StdioTransport`] and its main loop `tokio::select!`s
//! on [`StdioTransport::read_line`] alongside shutdown signals, writing each
//! response inline on the same task. Reads and writes are therefore sequential,
//! not concurrent — the `&mut self` methods reflect that single-owner model.
//!
//! Because `read_line` is polled inside `select!`, it may be cancelled when a
//! shutdown signal fires; that is safe here because the server then exits
//! rather than resuming the partially-read line.

use std::io;

use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};

use crate::mcp::protocol::{JsonRpcError, JsonRpcResponse, OutgoingNotification};

/// A stdio-based MCP transport.
///
/// Handles reading JSON-RPC messages from stdin and writing responses to stdout.
pub struct StdioTransport {
    /// Buffered reader for stdin.
    reader: BufReader<tokio::io::Stdin>,
    /// Handle for stdout.
    writer: tokio::io::Stdout,
    /// Test-only capture sink. When set, [`StdioTransport::write_raw`] appends
    /// framed messages here instead of writing to the real stdout, so unit
    /// tests can drive and assert on the write path without touching the
    /// process's stdout (which both pollutes the test runner output and relies
    /// on `tokio::io::stdout()`'s blocking-thread teardown — an intermittent
    /// source of hangs on Windows CI). Compiled out of non-test builds.
    #[cfg(test)]
    test_sink: Option<std::sync::Arc<std::sync::Mutex<Vec<u8>>>>,
}

impl StdioTransport {
    /// Creates a new stdio transport.
    #[must_use]
    pub fn new() -> Self {
        Self {
            reader: BufReader::new(tokio::io::stdin()),
            writer: tokio::io::stdout(),
            #[cfg(test)]
            test_sink: None,
        }
    }

    /// Redirects all subsequent writes to an in-memory buffer instead of the
    /// real stdout, returning a shared handle to inspect what was written.
    ///
    /// Test-only: lets unit tests that exercise the write path (e.g. the
    /// JSON-RPC dispatch tests) assert on the bytes written without going
    /// through `tokio::io::stdout()`.
    #[cfg(test)]
    pub(crate) fn capture_output(&mut self) -> std::sync::Arc<std::sync::Mutex<Vec<u8>>> {
        let sink = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        self.test_sink = Some(std::sync::Arc::clone(&sink));
        sink
    }

    /// Reads the next message line from stdin.
    ///
    /// Returns `None` if stdin is closed (EOF). A trailing `\n` or `\r\n` is
    /// stripped before the line is returned.
    ///
    /// The read is intentionally unbounded: `repo_push` carries its git bundle
    /// as base64 inline in a single JSON-RPC request (up to the configured
    /// bundle-size limit, on the order of 1 GiB), so one message line can
    /// legitimately be hundreds of megabytes. Capping the line length here
    /// would break large pushes.
    ///
    /// # Errors
    ///
    /// Returns an error if reading from stdin fails, including a line that is
    /// not valid UTF-8.
    pub async fn read_line(&mut self) -> io::Result<Option<String>> {
        read_message_line(&mut self.reader).await
    }

    /// Writes a JSON-RPC response to stdout.
    ///
    /// The response is serialised to JSON and terminated with a newline.
    ///
    /// # Errors
    ///
    /// Returns an error if serialisation or writing fails.
    pub async fn write_response(&mut self, response: &JsonRpcResponse) -> io::Result<()> {
        let json = serde_json::to_string(response)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        self.write_raw(&json).await
    }

    /// Writes a JSON-RPC error to stdout.
    ///
    /// # Errors
    ///
    /// Returns an error if serialisation or writing fails.
    pub async fn write_error(&mut self, error: &JsonRpcError) -> io::Result<()> {
        let json = serde_json::to_string(error)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        self.write_raw(&json).await
    }

    /// Writes a JSON-RPC notification to stdout.
    ///
    /// Used for sending progress updates and other server-initiated messages.
    ///
    /// # Errors
    ///
    /// Returns an error if serialisation or writing fails.
    pub async fn write_notification(
        &mut self,
        notification: &OutgoingNotification,
    ) -> io::Result<()> {
        let json = serde_json::to_string(notification)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        self.write_raw(&json).await
    }

    /// Writes a raw JSON string to stdout with newline termination.
    ///
    /// # Errors
    ///
    /// Returns an error if writing fails.
    async fn write_raw(&mut self, json: &str) -> io::Result<()> {
        #[cfg(test)]
        if let Some(sink) = &self.test_sink {
            // In-memory capture: append the framed message synchronously. The
            // framing matches `write_message_line` (message + '\n'), and there
            // is no real I/O, so the lock is never held across an await. The
            // guard is scoped tightly so it is released before returning.
            debug_assert!(
                !json.contains('\n'),
                "JSON message must not contain embedded newlines"
            );
            {
                let mut buf = sink.lock().expect("test_sink mutex poisoned");
                buf.extend_from_slice(json.as_bytes());
                buf.push(b'\n');
            }
            return Ok(());
        }

        write_message_line(&mut self.writer, json).await
    }

    /// Writes an arbitrary JSON value to stdout.
    ///
    /// Used for sending messages that don't fit the standard response types.
    ///
    /// # Errors
    ///
    /// Returns an error if serialisation or writing fails.
    pub async fn write_json(&mut self, value: &serde_json::Value) -> io::Result<()> {
        let json = serde_json::to_string(value)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        self.write_raw(&json).await
    }
}

impl Default for StdioTransport {
    fn default() -> Self {
        Self::new()
    }
}

/// Strips a single trailing line terminator (`\n` or `\r\n`) in place.
///
/// A lone `\r` (old-Mac style) is not a JSON-RPC delimiter, so it is left
/// intact.
fn strip_trailing_newline(line: &mut String) {
    if line.ends_with('\n') {
        line.pop();
        if line.ends_with('\r') {
            line.pop();
        }
    }
}

/// Reads one newline-delimited message from `reader`, returning `None` at EOF.
///
/// Generic over the reader so the line framing can be unit-tested without real
/// stdin; [`StdioTransport::read_line`] delegates here. See that method for why
/// the read is intentionally unbounded.
async fn read_message_line<R>(reader: &mut R) -> io::Result<Option<String>>
where
    R: AsyncBufRead + Unpin,
{
    let mut line = String::new();
    if reader.read_line(&mut line).await? == 0 {
        // EOF - stream closed
        return Ok(None);
    }

    strip_trailing_newline(&mut line);
    Ok(Some(line))
}

/// Writes `json` to `writer`, terminates it with a newline, and flushes.
///
/// Generic over the writer so the framing can be unit-tested without real
/// stdout; [`StdioTransport::write_raw`] delegates here.
async fn write_message_line<W>(writer: &mut W, json: &str) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    // MCP spec: messages must not contain embedded newlines
    debug_assert!(
        !json.contains('\n'),
        "JSON message must not contain embedded newlines"
    );

    writer.write_all(json.as_bytes()).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::protocol::RequestId;

    #[test]
    fn transport_default() {
        // Just ensure Default is implemented and doesn't panic
        let _transport = StdioTransport::default();
    }

    #[tokio::test]
    async fn serialise_response_no_newlines() {
        // Verify our JSON serialisation doesn't produce embedded newlines
        let response = JsonRpcResponse::success(
            RequestId::Number(1),
            serde_json::json!({
                "message": "hello world",
                "nested": {"key": "value"}
            }),
        );

        let json = serde_json::to_string(&response).unwrap();
        assert!(
            !json.contains('\n'),
            "Serialised JSON should not contain newlines"
        );
    }

    #[tokio::test]
    async fn serialise_error_no_newlines() {
        let error = JsonRpcError::method_not_found(RequestId::Number(1), "test/method");

        let json = serde_json::to_string(&error).unwrap();
        assert!(
            !json.contains('\n'),
            "Serialised JSON should not contain newlines"
        );
    }

    #[test]
    fn strip_trailing_newline_handles_lf_crlf_and_bare() {
        let mut lf = String::from("hello\n");
        strip_trailing_newline(&mut lf);
        assert_eq!(lf, "hello");

        let mut crlf = String::from("hello\r\n");
        strip_trailing_newline(&mut crlf);
        assert_eq!(crlf, "hello");

        // No terminator: unchanged.
        let mut bare = String::from("hello");
        strip_trailing_newline(&mut bare);
        assert_eq!(bare, "hello");

        // A lone CR is not a JSON-RPC delimiter and is preserved.
        let mut cr = String::from("hello\r");
        strip_trailing_newline(&mut cr);
        assert_eq!(cr, "hello\r");

        // Empty line stays empty.
        let mut empty = String::new();
        strip_trailing_newline(&mut empty);
        assert_eq!(empty, "");
    }

    #[tokio::test]
    async fn read_message_line_strips_lf_and_crlf_across_messages() {
        let mut reader = BufReader::new(&b"first\nsecond\r\n"[..]);

        assert_eq!(
            read_message_line(&mut reader).await.unwrap(),
            Some("first".to_string())
        );
        assert_eq!(
            read_message_line(&mut reader).await.unwrap(),
            Some("second".to_string())
        );
        // Stream exhausted -> EOF.
        assert_eq!(read_message_line(&mut reader).await.unwrap(), None);
    }

    #[tokio::test]
    async fn read_message_line_returns_final_line_without_newline() {
        // A final line with no trailing newline is still returned, then EOF.
        let mut reader = BufReader::new(&b"no-newline"[..]);
        assert_eq!(
            read_message_line(&mut reader).await.unwrap(),
            Some("no-newline".to_string())
        );
        assert_eq!(read_message_line(&mut reader).await.unwrap(), None);
    }

    #[tokio::test]
    async fn read_message_line_empty_input_is_eof() {
        let mut reader = BufReader::new(&b""[..]);
        assert_eq!(read_message_line(&mut reader).await.unwrap(), None);
    }

    #[tokio::test]
    async fn read_message_line_preserves_empty_message() {
        // A bare newline is an empty (but present) message, not EOF.
        let mut reader = BufReader::new(&b"\n"[..]);
        assert_eq!(
            read_message_line(&mut reader).await.unwrap(),
            Some(String::new())
        );
        assert_eq!(read_message_line(&mut reader).await.unwrap(), None);
    }

    #[tokio::test]
    async fn write_message_line_appends_single_newline() {
        let mut buf: Vec<u8> = Vec::new();
        write_message_line(&mut buf, r#"{"jsonrpc":"2.0"}"#)
            .await
            .unwrap();
        assert_eq!(String::from_utf8(buf).unwrap(), "{\"jsonrpc\":\"2.0\"}\n");
    }

    #[tokio::test]
    async fn write_message_line_frames_each_message_separately() {
        let mut buf: Vec<u8> = Vec::new();
        write_message_line(&mut buf, "a").await.unwrap();
        write_message_line(&mut buf, "b").await.unwrap();
        assert_eq!(String::from_utf8(buf).unwrap(), "a\nb\n");
    }

    #[tokio::test]
    async fn capture_output_records_framed_writes_in_memory() {
        // With capture enabled, write_response/write_error go to the in-memory
        // sink (newline-framed) instead of the real stdout.
        let mut transport = StdioTransport::new();
        let sink = transport.capture_output();

        let response = JsonRpcResponse::success(RequestId::Number(1), serde_json::json!({}));
        transport.write_response(&response).await.unwrap();
        let error = JsonRpcError::method_not_found(RequestId::Number(2), "no/such");
        transport.write_error(&error).await.unwrap();

        let written = String::from_utf8(sink.lock().unwrap().clone()).unwrap();
        // Two newline-framed messages.
        assert_eq!(written.matches('\n').count(), 2);
        assert!(written.contains("\"result\"") && written.contains("\"id\":1"));
        assert!(written.contains("\"error\"") && written.contains("-32601"));
    }
}
