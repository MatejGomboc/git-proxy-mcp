//! MCP server implementation.
//!
//! This module implements the MCP server lifecycle:
//!
//! 1. **Initialisation**: Capability negotiation and version agreement
//! 2. **Operation**: Handling tool calls and other requests
//! 3. **Shutdown**: Graceful connection termination
//!
//! # Lifecycle Flow
//!
//! ```text
//! Client                     Server
//!   │                          │
//!   ├─── initialize ──────────▶│
//!   │                          │
//!   │◀── initialize result ────┤
//!   │                          │
//!   ├─── initialized ─────────▶│
//!   │    (notification)        │
//!   │                          │
//!   │      [Operation Phase]   │
//!   │                          │
//!   ├─── tools/list ──────────▶│
//!   │◀── tools list ───────────┤
//!   │                          │
//!   ├─── tools/call ──────────▶│
//!   │◀── call result ──────────┤
//!   │                          │
//!   │      [Shutdown]          │
//!   │                          │
//!   ├─── (close stdin) ───────▶│
//!   │                          │ exit
//! ```

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::git::command::GitCommand;
use crate::git::executor::GitExecutor;
use crate::mcp::protocol::{
    ErrorCode, IncomingMessage, JsonRpcError, JsonRpcErrorData, JsonRpcNotification,
    JsonRpcRequest, JsonRpcResponse, RequestId, MCP_PROTOCOL_VERSION, SERVER_NAME,
};
use crate::mcp::transport::StdioTransport;
use crate::security::{
    AuditEvent, AuditLogger, BranchGuard, PushGuard, RateLimiter, RepoFilter, SecurityGuard,
    ShutdownReason,
};

/// Server state in the MCP lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerState {
    /// Waiting for initialize request.
    AwaitingInit,
    /// Initialize received, waiting for initialized notification.
    Initialising,
    /// Ready for normal operation.
    Running,
    /// Shutdown in progress.
    ShuttingDown,
}

/// Server capabilities advertised during initialisation.
#[derive(Debug, Clone, Serialize)]
pub struct ServerCapabilities {
    /// Tool-related capabilities.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<ToolCapabilities>,
}

impl Default for ServerCapabilities {
    fn default() -> Self {
        Self {
            tools: Some(ToolCapabilities::default()),
        }
    }
}

/// Tool-specific capabilities.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ToolCapabilities {
    /// Whether the tool list can change during the session.
    #[serde(rename = "listChanged", skip_serializing_if = "std::ops::Not::not")]
    pub list_changed: bool,
}

/// Server information for initialisation response.
#[derive(Debug, Clone, Serialize)]
pub struct ServerInfo {
    /// Server name.
    pub name: String,
    /// Server version.
    pub version: String,
}

impl Default for ServerInfo {
    fn default() -> Self {
        Self {
            name: SERVER_NAME.to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }
}

/// Client information received during initialisation.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientInfo {
    /// Client name.
    pub name: String,
    /// Client version.
    #[serde(default)]
    pub version: Option<String>,
}

/// Parameters for the initialize request.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    /// Protocol version requested by client.
    pub protocol_version: String,
    /// Client capabilities.
    #[serde(default)]
    pub capabilities: Value,
    /// Client information.
    #[serde(default)]
    pub client_info: Option<ClientInfo>,
}

/// A tool definition for tools/list response.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDefinition {
    /// Unique tool name.
    pub name: String,
    /// Human-readable description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// JSON Schema for the tool's input parameters.
    pub input_schema: Value,
}

/// Parameters for tools/call request.
#[derive(Debug, Clone, Deserialize)]
pub struct ToolCallParams {
    /// Name of the tool to call.
    pub name: String,
    /// Arguments for the tool.
    #[serde(default)]
    pub arguments: Value,
}

/// Content item in a tool call response.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ToolContent {
    /// Text content.
    Text {
        /// The text content.
        text: String,
    },
}

/// Result of a tool call.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallResult {
    /// Content returned by the tool.
    pub content: Vec<ToolContent>,
    /// Whether the tool call resulted in an error.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub is_error: bool,
}

impl ToolCallResult {
    /// Creates a successful text result.
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            content: vec![ToolContent::Text { text: text.into() }],
            is_error: false,
        }
    }

    /// Creates an error text result.
    ///
    /// Per MCP spec, tool errors are reported in the result, not as protocol errors.
    #[must_use]
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            content: vec![ToolContent::Text {
                text: message.into(),
            }],
            is_error: true,
        }
    }
}

/// Configuration for security guards.
#[derive(Debug, Clone, Default)]
pub struct SecurityConfig {
    /// Whether force push is allowed.
    pub allow_force_push: bool,
    /// Protected branch names.
    pub protected_branches: Vec<String>,
    /// Repository allowlist (if set, only these repos are allowed).
    pub repo_allowlist: Option<Vec<String>>,
    /// Repository blocklist.
    pub repo_blocklist: Option<Vec<String>>,
}

/// The MCP server.
pub struct McpServer {
    /// Current server state.
    state: ServerState,
    /// The transport layer.
    transport: StdioTransport,
    /// Negotiated protocol version (set after initialisation).
    protocol_version: Option<String>,
    /// Git command executor.
    executor: Arc<GitExecutor>,
    /// Branch protection guard.
    branch_guard: BranchGuard,
    /// Push protection guard.
    push_guard: PushGuard,
    /// Repository filter.
    repo_filter: RepoFilter,
    /// Rate limiter.
    rate_limiter: RateLimiter,
    /// Audit logger.
    audit_logger: Arc<AuditLogger>,
}

impl McpServer {
    /// Creates a new MCP server with the given dependencies.
    ///
    /// # Arguments
    ///
    /// * `executor` — Git command executor with credentials
    /// * `security_config` — Security settings from configuration
    /// * `audit_logger` — Audit logger for recording operations
    #[must_use]
    pub fn new(
        executor: GitExecutor,
        security_config: SecurityConfig,
        audit_logger: AuditLogger,
    ) -> Self {
        // Build branch guard from protected branches
        let branch_guard = if security_config.protected_branches.is_empty() {
            BranchGuard::with_defaults()
        } else {
            BranchGuard::new(security_config.protected_branches)
        };

        // Build push guard
        let push_guard = PushGuard::new(security_config.allow_force_push);

        // Build repo filter
        let mut repo_filter = if security_config.repo_allowlist.is_some() {
            RepoFilter::allowlist_mode()
        } else {
            RepoFilter::blocklist_mode()
        };

        if let Some(allowlist) = security_config.repo_allowlist {
            for pattern in allowlist {
                repo_filter.allow(pattern);
            }
        }

        if let Some(blocklist) = security_config.repo_blocklist {
            for pattern in blocklist {
                repo_filter.block(pattern);
            }
        }

        Self {
            state: ServerState::AwaitingInit,
            transport: StdioTransport::new(),
            protocol_version: None,
            executor: Arc::new(executor),
            branch_guard,
            push_guard,
            repo_filter,
            rate_limiter: RateLimiter::default_for_ai(),
            audit_logger: Arc::new(audit_logger),
        }
    }

    /// Returns the current server state.
    #[must_use]
    pub const fn state(&self) -> ServerState {
        self.state
    }

    /// Runs the MCP server main loop with graceful shutdown handling.
    ///
    /// This method blocks until:
    /// - The client closes the connection (stdin closed)
    /// - A shutdown signal is received (SIGINT/SIGTERM on Unix, Ctrl+C on Windows)
    /// - An unrecoverable error occurs
    ///
    /// # Errors
    ///
    /// Returns an error if transport I/O fails.
    pub async fn run(&mut self) -> std::io::Result<()> {
        let shutdown_reason = self.run_with_shutdown().await?;
        self.audit_logger
            .log_silent(&AuditEvent::server_stopped(shutdown_reason));
        Ok(())
    }

    /// Runs the main loop and returns the shutdown reason.
    #[cfg(unix)]
    async fn run_with_shutdown(&mut self) -> std::io::Result<ShutdownReason> {
        use tokio::signal::unix::{signal, SignalKind};

        let mut sigint = signal(SignalKind::interrupt()).map_err(std::io::Error::other)?;
        let mut sigterm = signal(SignalKind::terminate()).map_err(std::io::Error::other)?;

        loop {
            tokio::select! {
                _ = sigint.recv() => {
                    tracing::info!("Received SIGINT, initiating graceful shutdown");
                    self.state = ServerState::ShuttingDown;
                    return Ok(ShutdownReason::SigInt);
                }

                _ = sigterm.recv() => {
                    tracing::info!("Received SIGTERM, initiating graceful shutdown");
                    self.state = ServerState::ShuttingDown;
                    return Ok(ShutdownReason::SigTerm);
                }

                line_result = self.transport.read_line() => {
                    if let Some(reason) = self.handle_transport_result(line_result).await? {
                        return Ok(reason);
                    }
                }
            }
        }
    }

    /// Runs the main loop and returns the shutdown reason.
    #[cfg(windows)]
    async fn run_with_shutdown(&mut self) -> std::io::Result<ShutdownReason> {
        let ctrl_c = tokio::signal::ctrl_c();
        tokio::pin!(ctrl_c);

        loop {
            tokio::select! {
                _ = &mut ctrl_c => {
                    tracing::info!("Received Ctrl+C, initiating graceful shutdown");
                    self.state = ServerState::ShuttingDown;
                    return Ok(ShutdownReason::SigInt);
                }

                line_result = self.transport.read_line() => {
                    if let Some(reason) = self.handle_transport_result(line_result).await? {
                        return Ok(reason);
                    }
                }
            }
        }
    }

    /// Handles the result from transport read and message processing.
    ///
    /// Returns `Some(reason)` if the server should shut down, `None` to continue.
    async fn handle_transport_result(
        &mut self,
        line_result: std::io::Result<Option<String>>,
    ) -> std::io::Result<Option<ShutdownReason>> {
        let Some(line) = line_result? else {
            // EOF - client closed connection
            self.state = ServerState::ShuttingDown;
            return Ok(Some(ShutdownReason::ClientDisconnected));
        };

        // Skip empty lines
        if line.trim().is_empty() {
            return Ok(None);
        }

        // Parse and handle the message
        self.handle_line(&line).await?;

        // Check if we should exit (e.g., from a shutdown notification)
        if self.state == ServerState::ShuttingDown {
            return Ok(Some(ShutdownReason::ClientDisconnected));
        }

        Ok(None)
    }

    /// Handles a single line of input.
    async fn handle_line(&mut self, line: &str) -> std::io::Result<()> {
        use crate::mcp::protocol::parse_message;

        match parse_message(line) {
            Ok(msg) => self.handle_message(msg).await,
            Err(error) => {
                self.transport.write_error(&error).await?;
                Ok(())
            }
        }
    }

    /// Handles a parsed incoming message.
    async fn handle_message(&mut self, msg: IncomingMessage) -> std::io::Result<()> {
        match msg {
            IncomingMessage::Request(req) => self.handle_request(req).await,
            IncomingMessage::Notification(ref notif) => {
                self.handle_notification(notif);
                Ok(())
            }
        }
    }

    /// Handles an incoming request.
    async fn handle_request(&mut self, req: JsonRpcRequest) -> std::io::Result<()> {
        let response = match req.method.as_str() {
            "initialize" => self.handle_initialize(&req),
            "tools/list" => self.handle_tools_list(&req),
            "tools/call" => self.handle_tools_call(&req).await,
            "ping" => Ok(Self::handle_ping(&req)),
            _ => Err(JsonRpcError::method_not_found(req.id.clone(), &req.method)),
        };

        match response {
            Ok(resp) => self.transport.write_response(&resp).await,
            Err(error) => self.transport.write_error(&error).await,
        }
    }

    /// Handles an incoming notification.
    fn handle_notification(&mut self, notif: &JsonRpcNotification) {
        if notif.method == "notifications/initialized" && self.state == ServerState::Initialising {
            self.state = ServerState::Running;
        }
        // All other notifications (including unknown ones) are ignored per JSON-RPC spec
    }

    /// Handles the initialize request.
    fn handle_initialize(&mut self, req: &JsonRpcRequest) -> Result<JsonRpcResponse, JsonRpcError> {
        // Must be in AwaitingInit state
        if self.state != ServerState::AwaitingInit {
            return Err(JsonRpcError::new(
                Some(req.id.clone()),
                JsonRpcErrorData::with_message(
                    ErrorCode::InvalidRequest,
                    "Server already initialised",
                ),
            ));
        }

        // Parse initialise params
        let _params: InitializeParams = req
            .params
            .as_ref()
            .map(|p| serde_json::from_value(p.clone()))
            .transpose()
            .map_err(|e| {
                JsonRpcError::invalid_params(
                    req.id.clone(),
                    format!("Invalid initialize params: {e}"),
                )
            })?
            .ok_or_else(|| {
                JsonRpcError::invalid_params(req.id.clone(), "Missing initialize params")
            })?;

        // Check protocol version
        // We currently only support one version, so we always return our version
        // The client will disconnect if it doesn't support our version
        let negotiated_version = MCP_PROTOCOL_VERSION.to_string();

        self.protocol_version = Some(negotiated_version.clone());
        self.state = ServerState::Initialising;

        let result = json!({
            "protocolVersion": negotiated_version,
            "capabilities": ServerCapabilities::default(),
            "serverInfo": ServerInfo::default(),
        });

        Ok(JsonRpcResponse::success(req.id.clone(), result))
    }

    /// Handles the tools/list request.
    fn handle_tools_list(&self, req: &JsonRpcRequest) -> Result<JsonRpcResponse, JsonRpcError> {
        self.require_running(&req.id)?;

        let tools = Self::get_tool_definitions();

        let result = json!({
            "tools": tools,
        });

        Ok(JsonRpcResponse::success(req.id.clone(), result))
    }

    /// Handles the tools/call request.
    async fn handle_tools_call(
        &self,
        req: &JsonRpcRequest,
    ) -> Result<JsonRpcResponse, JsonRpcError> {
        self.require_running(&req.id)?;

        let params: ToolCallParams = req
            .params
            .as_ref()
            .map(|p| serde_json::from_value(p.clone()))
            .transpose()
            .map_err(|e| {
                JsonRpcError::invalid_params(
                    req.id.clone(),
                    format!("Invalid tool call params: {e}"),
                )
            })?
            .ok_or_else(|| {
                JsonRpcError::invalid_params(req.id.clone(), "Missing tool call params")
            })?;

        let result = match params.name.as_str() {
            "git" => self.call_git_tool(&params.arguments).await,
            _ => ToolCallResult::error(format!("Unknown tool: {}", params.name)),
        };

        // Serialise the result. This should never fail for our types (String, bool, Vec)
        // but we handle it gracefully to avoid panicking in production.
        let result_value = serde_json::to_value(&result).map_err(|e| {
            tracing::error!(error = %e, "Failed to serialise tool call result");
            JsonRpcError::new(
                Some(req.id.clone()),
                JsonRpcErrorData::with_message(
                    ErrorCode::InternalError,
                    "Internal error: failed to serialise result",
                ),
            )
        })?;

        Ok(JsonRpcResponse::success(req.id.clone(), result_value))
    }

    /// Handles the ping request.
    fn handle_ping(req: &JsonRpcRequest) -> JsonRpcResponse {
        // Ping is allowed in any state
        JsonRpcResponse::success(req.id.clone(), json!({}))
    }

    /// Ensures the server is in the Running state.
    fn require_running(&self, id: &RequestId) -> Result<(), JsonRpcError> {
        if self.state != ServerState::Running {
            return Err(JsonRpcError::new(
                Some(id.clone()),
                JsonRpcErrorData::with_message(ErrorCode::InvalidRequest, "Server not initialised"),
            ));
        }
        Ok(())
    }

    /// Returns the list of available tools.
    fn get_tool_definitions() -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            name: "git".to_string(),
            description: Some(
                "Execute remote Git commands using your existing Git credential configuration. \
                 Only remote operations are supported: clone, fetch, pull, push, ls-remote. \
                 Local commands (status, log, diff, commit, etc.) should be run directly. \
                 Authentication is handled by your system's credential helpers and SSH agent."
                    .to_string(),
            ),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "enum": ["clone", "fetch", "ls-remote", "pull", "push"],
                        "description": "The remote Git command to execute"
                    },
                    "args": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Arguments to pass to the Git command"
                    },
                    "cwd": {
                        "type": "string",
                        "description": "Working directory for the Git command (optional)"
                    }
                },
                "required": ["command"]
            }),
        }]
    }

    /// Applies all security guards to a command.
    ///
    /// Returns `Some(reason)` if the command should be blocked.
    fn check_security_guards(&self, command: &str, args: &[String]) -> Option<String> {
        // Check branch guard
        if let Some(reason) = self.branch_guard.check(command, args).reason() {
            return Some(reason.to_string());
        }

        // Check push guard
        if let Some(reason) = self.push_guard.check(command, args).reason() {
            return Some(reason.to_string());
        }

        // Check repo filter
        if let Some(reason) = self.repo_filter.check(command, args).reason() {
            return Some(reason.to_string());
        }

        None
    }

    /// Formats command output into a response string.
    fn format_output(output: &crate::git::executor::CommandOutput, command: &str) -> String {
        let mut response_text = String::new();

        if !output.stdout.is_empty() {
            response_text.push_str(&output.stdout);
        }

        if !output.stderr.is_empty() {
            if !response_text.is_empty() {
                response_text.push_str("\n\n--- stderr ---\n");
            }
            response_text.push_str(&output.stderr);
        }

        // Add warnings
        for warning in &output.warnings {
            response_text.push_str("\n\n⚠️ ");
            response_text.push_str(warning);
        }

        if response_text.is_empty() {
            response_text = format!("Command 'git {command}' completed successfully.");
        }

        response_text
    }

    /// Executes the git tool.
    ///
    /// This method:
    /// 1. Parses and validates the command
    /// 2. Applies security guards (rate limiting, branch protection, repo filtering)
    /// 3. Executes the command with credential injection
    /// 4. Logs the operation to the audit log
    /// 5. Returns sanitised output
    async fn call_git_tool(&self, arguments: &Value) -> ToolCallResult {
        let start_time = Instant::now();

        // Extract command from arguments
        let command_str = match arguments.get("command").and_then(Value::as_str) {
            Some(cmd) if !cmd.is_empty() => cmd,
            _ => return ToolCallResult::error("Missing required 'command' argument"),
        };

        // Extract args
        let args: Vec<String> = arguments
            .get("args")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(Value::as_str)
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();

        // Extract working directory
        let working_dir: Option<PathBuf> = arguments
            .get("cwd")
            .and_then(Value::as_str)
            .map(PathBuf::from);

        // Check rate limiter first
        if !self.rate_limiter.try_acquire() {
            self.audit_logger
                .log_silent(&AuditEvent::rate_limit_exceeded(
                    command_str,
                    args.clone(),
                    working_dir.clone(),
                ));
            return ToolCallResult::error(
                "Rate limit exceeded. Please wait before sending more Git commands.",
            );
        }

        // Parse and validate the command
        let git_command = match GitCommand::new(command_str, args.clone(), working_dir.clone()) {
            Ok(cmd) => cmd,
            Err(e) => {
                self.audit_logger.log_silent(&AuditEvent::command_blocked(
                    command_str,
                    args,
                    working_dir,
                    e.to_string(),
                ));
                return ToolCallResult::error(format!("Invalid command: {e}"));
            }
        };

        // Apply security guards
        let args_for_guard: Vec<String> = git_command.args().to_vec();
        if let Some(reason) = self.check_security_guards(command_str, &args_for_guard) {
            self.audit_logger.log_silent(&AuditEvent::command_blocked(
                command_str,
                args,
                working_dir,
                &reason,
            ));
            return ToolCallResult::error(reason);
        }

        // Execute the command
        let output = match self.executor.execute(&git_command).await {
            Ok(output) => output,
            Err(e) => {
                let duration = start_time.elapsed();
                self.audit_logger.log_silent(&AuditEvent::command_success(
                    command_str,
                    args,
                    working_dir,
                    duration,
                    -1,
                ));
                return ToolCallResult::error(format!("Execution failed: {e}"));
            }
        };

        // Log the operation
        let duration = start_time.elapsed();
        self.audit_logger.log_silent(&AuditEvent::command_success(
            command_str,
            args,
            working_dir,
            duration,
            output.exit_code,
        ));

        // Format and return the response
        let response_text = Self::format_output(&output, command_str);
        if output.success {
            ToolCallResult::text(response_text)
        } else {
            ToolCallResult::error(format!(
                "Command failed with exit code {}:\n{}",
                output.exit_code, response_text
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Creates a test server with minimal configuration.
    fn create_test_server() -> McpServer {
        let executor = GitExecutor::new();
        let security_config = SecurityConfig::default();
        let audit_logger = AuditLogger::disabled();

        McpServer::new(executor, security_config, audit_logger)
    }

    #[test]
    fn server_initial_state() {
        let server = create_test_server();
        assert_eq!(server.state(), ServerState::AwaitingInit);
    }

    #[test]
    fn tool_definitions_valid() {
        let tools = McpServer::get_tool_definitions();

        assert!(!tools.is_empty());

        for tool in &tools {
            assert!(!tool.name.is_empty());
            assert!(tool.input_schema.is_object());
        }
    }

    #[test]
    fn tool_call_result_text() {
        let result = ToolCallResult::text("Hello, world!");
        assert!(!result.is_error);
        assert_eq!(result.content.len(), 1);

        match &result.content[0] {
            ToolContent::Text { text } => assert_eq!(text, "Hello, world!"),
        }
    }

    #[test]
    fn tool_call_result_error() {
        let result = ToolCallResult::error("Something went wrong");
        assert!(result.is_error);
        assert_eq!(result.content.len(), 1);

        match &result.content[0] {
            ToolContent::Text { text } => assert_eq!(text, "Something went wrong"),
        }
    }

    #[test]
    fn server_capabilities_serialisation() {
        let caps = ServerCapabilities::default();
        let json = serde_json::to_value(&caps).unwrap();

        assert!(json.get("tools").is_some());
    }

    #[test]
    fn server_info_default() {
        let info = ServerInfo::default();
        assert_eq!(info.name, SERVER_NAME);
        assert!(!info.version.is_empty());
    }
}
