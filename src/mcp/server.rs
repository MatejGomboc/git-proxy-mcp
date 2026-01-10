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

use std::sync::Arc;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::mcp::protocol::{
    ErrorCode, IncomingMessage, JsonRpcError, JsonRpcErrorData, JsonRpcNotification,
    JsonRpcRequest, JsonRpcResponse, RequestId, MCP_PROTOCOL_VERSION, SERVER_NAME,
};
use crate::mcp::tools::{
    handle_repo_clone, handle_repo_clone_cancel, handle_repo_clone_chunk, handle_repo_clone_start,
    handle_repo_diff, handle_repo_pull, handle_repo_push, handle_repo_refs, RepoCloneArgs,
    RepoCloneCancelArgs, RepoCloneChunkArgs, RepoCloneStartArgs, RepoDiffArgs, RepoPullArgs,
    RepoPushArgs, RepoRefsArgs,
};
use crate::mcp::transport::StdioTransport;
use crate::security::{
    AuditEvent, AuditLogger, BranchGuard, PushGuard, RateLimiter, RepoFilter, SecurityGuard,
    ShutdownReason,
};
use crate::streaming::chunked::StreamingSessionManager;

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
    /// Rate limiting: maximum operations in a burst.
    pub rate_limit_max_burst: u64,
    /// Rate limiting: sustained operations per second.
    pub rate_limit_refill_rate: f64,
}

/// Git identity configuration for AI-assisted commits.
///
/// This identity is communicated to the AI during initialization so they
/// can configure their local Git to use it for commits. This allows
/// clear separation between AI-made commits and human commits.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitIdentity {
    /// Name for commit author/committer (e.g., "Claude AI").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Email for commit author/committer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

impl GitIdentity {
    /// Returns true if both name and email are set.
    #[must_use]
    pub const fn is_configured(&self) -> bool {
        self.name.is_some() && self.email.is_some()
    }

    /// Returns true if at least one field is set.
    #[must_use]
    pub const fn is_partial(&self) -> bool {
        self.name.is_some() || self.email.is_some()
    }
}

/// The MCP server.
pub struct McpServer {
    /// Current server state.
    state: ServerState,
    /// The transport layer.
    transport: StdioTransport,
    /// Negotiated protocol version (set after initialisation).
    protocol_version: Option<String>,
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
    /// Streaming session manager for Tier 2 chunked streaming.
    streaming_sessions: StreamingSessionManager,
    /// Git identity for AI-assisted commits.
    git_identity: GitIdentity,
}

impl McpServer {
    /// Creates a new MCP server with the given dependencies.
    ///
    /// # Arguments
    ///
    /// * `security_config` — Security settings from configuration
    /// * `git_identity` — Git identity for AI-assisted commits
    /// * `audit_logger` — Audit logger for recording operations
    #[must_use]
    pub fn new(
        security_config: SecurityConfig,
        git_identity: GitIdentity,
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

        // Build rate limiter from config (0 means unlimited)
        let rate_limiter = if security_config.rate_limit_max_burst == 0 {
            RateLimiter::unlimited()
        } else {
            RateLimiter::new(
                security_config.rate_limit_max_burst,
                security_config.rate_limit_refill_rate,
            )
        };

        Self {
            state: ServerState::AwaitingInit,
            transport: StdioTransport::new(),
            protocol_version: None,
            branch_guard,
            push_guard,
            repo_filter,
            rate_limiter,
            audit_logger: Arc::new(audit_logger),
            streaming_sessions: StreamingSessionManager::new(),
            git_identity,
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
            "tools/call" => self.handle_tools_call(&req),
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

        // Build result with optional git identity
        let mut result = json!({
            "protocolVersion": negotiated_version,
            "capabilities": ServerCapabilities::default(),
            "serverInfo": ServerInfo::default(),
        });

        // Include git identity if configured (for AI to use when creating commits)
        if self.git_identity.is_partial() {
            result["gitIdentity"] = serde_json::to_value(&self.git_identity).unwrap_or(Value::Null);
        }

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
    fn handle_tools_call(&self, req: &JsonRpcRequest) -> Result<JsonRpcResponse, JsonRpcError> {
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
            "repo/clone" => self.call_repo_clone_tool(&params.arguments),
            "repo/push" => self.call_repo_push_tool(&params.arguments),
            "repo/refs" => self.call_repo_refs_tool(&params.arguments),
            "repo/diff" => self.call_repo_diff_tool(&params.arguments),
            "repo/pull" => self.call_repo_pull_tool(&params.arguments),
            // Tier 2: Chunked streaming tools
            "repo/clone_start" => self.call_repo_clone_start_tool(&params.arguments),
            "repo/clone_chunk" => self.call_repo_clone_chunk_tool(&params.arguments),
            "repo/clone_cancel" => self.call_repo_clone_cancel_tool(&params.arguments),
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
    #[allow(clippy::too_many_lines)] // Tool definitions are naturally verbose
    fn get_tool_definitions() -> Vec<ToolDefinition> {
        vec![
            // Tier 1: Stream repository as tar.gz
            ToolDefinition {
                name: "repo/clone".to_string(),
                description: Some(
                    "Clone a repository and return it as a base64-encoded tar.gz archive. \
                     The repository is fetched using your local Git credentials (SSH agent or \
                     credential helpers) but no source files are written to your disk. \
                     Use this to get a complete repository snapshot that can be extracted \
                     on the AI's VM."
                        .to_string(),
                ),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "url": {
                            "type": "string",
                            "description": "Repository URL (https:// or git@)"
                        },
                        "branch": {
                            "type": "string",
                            "description": "Branch to clone (defaults to 'main')"
                        },
                        "depth": {
                            "type": "integer",
                            "description": "Shallow clone depth (1 = only latest commit)"
                        },
                        "sparse": {
                            "type": "array",
                            "items": {"type": "string"},
                            "description": "Sparse checkout patterns (glob syntax, e.g., 'src/**/*.rs')"
                        },
                        "exclude_binary": {
                            "type": "boolean",
                            "description": "Exclude binary files (files with null bytes or mostly non-printable chars). Useful for AI code review."
                        },
                        "max_file_size": {
                            "type": "integer",
                            "description": "Maximum file size in bytes. Files larger than this are skipped. Useful for excluding large assets."
                        },
                        "resolve_lfs": {
                            "type": "boolean",
                            "description": "Resolve Git LFS pointers to actual content. When enabled, LFS pointer files are replaced with their actual content."
                        },
                        "include_submodules": {
                            "type": "boolean",
                            "description": "Include submodule contents in the archive. When enabled, submodules are fetched and their files are included at their respective paths."
                        }
                    },
                    "required": ["url"]
                }),
            },
            // Tier 1: Push a git bundle to remote
            ToolDefinition {
                name: "repo/push".to_string(),
                description: Some(
                    "Push a git bundle to a remote repository. The AI creates a bundle using \
                     'git bundle create' and sends it base64-encoded. The MCP server unbundles \
                     and pushes using your local Git credentials. Protected branch guards apply."
                        .to_string(),
                ),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "bundle": {
                            "type": "string",
                            "description": "Base64-encoded git bundle created with 'git bundle create'"
                        },
                        "url": {
                            "type": "string",
                            "description": "Target repository URL (https:// or git@)"
                        },
                        "branch": {
                            "type": "string",
                            "description": "Target branch to push to"
                        },
                        "force": {
                            "type": "boolean",
                            "description": "Force push (use with caution, may be blocked by guards)"
                        }
                    },
                    "required": ["bundle", "url", "branch"]
                }),
            },
            // Tier 2: Start chunked clone
            ToolDefinition {
                name: "repo/clone_start".to_string(),
                description: Some(
                    "Start a chunked clone for large repositories. Returns a session ID that \
                     can be used with repo/clone_chunk to retrieve the data in pieces. \
                     Use this instead of repo/clone when working with large repositories \
                     to get progress updates and enable resume on failure."
                        .to_string(),
                ),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "url": {
                            "type": "string",
                            "description": "Repository URL (https:// or git@)"
                        },
                        "branch": {
                            "type": "string",
                            "description": "Branch to clone (defaults to 'main')"
                        },
                        "depth": {
                            "type": "integer",
                            "description": "Shallow clone depth (1 = only latest commit)"
                        },
                        "sparse": {
                            "type": "array",
                            "items": {"type": "string"},
                            "description": "Sparse checkout paths (glob patterns)"
                        },
                        "chunk_size": {
                            "type": "integer",
                            "description": "Chunk size in bytes (default: 1MB, max: 4MB)"
                        }
                    },
                    "required": ["url"]
                }),
            },
            // Tier 2: Get a chunk from streaming session
            ToolDefinition {
                name: "repo/clone_chunk".to_string(),
                description: Some(
                    "Get a chunk from a streaming clone session. Call repeatedly with \
                     incrementing chunk_index (starting from 0) until is_last is true. \
                     Concatenate all chunks to reconstruct the tar.gz archive."
                        .to_string(),
                ),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "session_id": {
                            "type": "string",
                            "description": "Session ID from repo/clone_start"
                        },
                        "chunk_index": {
                            "type": "integer",
                            "description": "Chunk index to retrieve (0-based)"
                        }
                    },
                    "required": ["session_id", "chunk_index"]
                }),
            },
            // Tier 2: Cancel a streaming session
            ToolDefinition {
                name: "repo/clone_cancel".to_string(),
                description: Some(
                    "Cancel a streaming clone session and free resources. Call this if \
                     you no longer need the remaining chunks. Sessions also auto-expire \
                     after 1 hour of inactivity."
                        .to_string(),
                ),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "session_id": {
                            "type": "string",
                            "description": "Session ID to cancel"
                        }
                    },
                    "required": ["session_id"]
                }),
            },
            // List remote refs without cloning
            ToolDefinition {
                name: "repo/refs".to_string(),
                description: Some(
                    "List branches and tags from a remote repository without cloning. \
                     Returns structured information about available branches, tags, and \
                     the default branch. Use this to explore a repository before cloning. \
                     Equivalent to 'git ls-remote' but with structured output."
                        .to_string(),
                ),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "url": {
                            "type": "string",
                            "description": "Repository URL (https:// or git@)"
                        }
                    },
                    "required": ["url"]
                }),
            },
            // Generate diff between commits
            ToolDefinition {
                name: "repo/diff".to_string(),
                description: Some(
                    "Generate a unified diff between two commits from a remote repository. \
                     Returns the diff text and statistics (files changed, insertions, deletions). \
                     Use this to review changes between commits, branches, or tags without \
                     downloading the entire repository content."
                        .to_string(),
                ),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "url": {
                            "type": "string",
                            "description": "Repository URL (https:// or git@)"
                        },
                        "base_commit": {
                            "type": "string",
                            "description": "Base commit reference (SHA, branch name, tag, or relative ref like HEAD~5)"
                        },
                        "head_commit": {
                            "type": "string",
                            "description": "Head commit reference (SHA, branch name, tag, or relative ref)"
                        }
                    },
                    "required": ["url", "base_commit", "head_commit"]
                }),
            },
            // Incremental sync (pull changes since known commit)
            ToolDefinition {
                name: "repo/pull".to_string(),
                description: Some(
                    "Fetch changes since a known commit for incremental sync. Returns a unified \
                     diff, a tar.gz archive of changed/added files, and a list of deleted files. \
                     Use this when the AI already has a repository and needs to sync updates \
                     without re-downloading everything."
                        .to_string(),
                ),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "url": {
                            "type": "string",
                            "description": "Repository URL (https:// or git@)"
                        },
                        "branch": {
                            "type": "string",
                            "description": "Branch name to sync (e.g., 'main')"
                        },
                        "since_commit": {
                            "type": "string",
                            "description": "Commit SHA that the AI already has (40-character hex)"
                        }
                    },
                    "required": ["url", "branch", "since_commit"]
                }),
            },
        ]
    }

    /// Calls the `repo/clone` tool.
    ///
    /// This tool clones a repository and returns it as a base64-encoded tar.gz.
    /// Source files are never written to the user's disk.
    fn call_repo_clone_tool(&self, arguments: &Value) -> ToolCallResult {
        use crate::git2_ops::auth::sanitize_url_for_logging;

        // Parse arguments
        let args: RepoCloneArgs = match serde_json::from_value(arguments.clone()) {
            Ok(a) => a,
            Err(e) => {
                return ToolCallResult::error(format!("Invalid arguments: {e}"));
            }
        };

        let sanitized_url = sanitize_url_for_logging(&args.url);

        // Check rate limiter
        if !self.rate_limiter.try_acquire() {
            self.audit_logger
                .log_silent(&AuditEvent::repo_clone_blocked(
                    &sanitized_url,
                    "rate limit exceeded",
                ));
            return ToolCallResult::error(
                "Rate limit exceeded. Please wait before sending more requests.",
            );
        }

        // Check repo filter
        if let Some(reason) = self
            .repo_filter
            .check("clone", std::slice::from_ref(&args.url))
            .reason()
        {
            self.audit_logger
                .log_silent(&AuditEvent::repo_clone_blocked(&sanitized_url, reason));
            return ToolCallResult::error(reason.to_string());
        }

        // Execute the clone with timing
        let start = Instant::now();
        match handle_repo_clone(args) {
            Ok(result) => {
                let duration = start.elapsed();
                self.audit_logger
                    .log_silent(&AuditEvent::repo_clone_success(
                        &sanitized_url,
                        &result.branch,
                        &result.commit,
                        result.file_count,
                        result.archive_size,
                        duration,
                    ));

                // Return the result as JSON text
                match serde_json::to_string_pretty(&result) {
                    Ok(json) => ToolCallResult::text(json),
                    Err(e) => ToolCallResult::error(format!("Failed to serialize result: {e}")),
                }
            }
            Err(e) => {
                self.audit_logger.log_silent(&AuditEvent::repo_clone_failed(
                    &sanitized_url,
                    e.to_string(),
                ));
                ToolCallResult::error(e.to_string())
            }
        }
    }

    /// Calls the `repo/push` tool.
    ///
    /// This tool receives a git bundle and pushes it to a remote repository.
    /// Only the bundle file touches disk (not source files).
    fn call_repo_push_tool(&self, arguments: &Value) -> ToolCallResult {
        use crate::git2_ops::auth::sanitize_url_for_logging;

        // Parse arguments
        let args: RepoPushArgs = match serde_json::from_value(arguments.clone()) {
            Ok(a) => a,
            Err(e) => {
                return ToolCallResult::error(format!("Invalid arguments: {e}"));
            }
        };

        let sanitized_url = sanitize_url_for_logging(&args.url);

        // Check rate limiter
        if !self.rate_limiter.try_acquire() {
            self.audit_logger.log_silent(&AuditEvent::repo_push_blocked(
                &sanitized_url,
                "rate limit exceeded",
            ));
            return ToolCallResult::error(
                "Rate limit exceeded. Please wait before sending more requests.",
            );
        }

        // Check repo filter
        if let Some(reason) = self
            .repo_filter
            .check("push", std::slice::from_ref(&args.url))
            .reason()
        {
            self.audit_logger
                .log_silent(&AuditEvent::repo_push_blocked(&sanitized_url, reason));
            return ToolCallResult::error(reason.to_string());
        }

        // Check branch guard (protected branches)
        if let Some(reason) = self
            .branch_guard
            .check("push", std::slice::from_ref(&args.branch))
            .reason()
        {
            self.audit_logger.log_silent(&AuditEvent::repo_push_blocked(
                &sanitized_url,
                format!("protected branch: {reason}"),
            ));
            return ToolCallResult::error(reason.to_string());
        }

        // Check push guard (force push)
        if args.force {
            if let Some(reason) = self
                .push_guard
                .check("push", &["--force".to_string()])
                .reason()
            {
                self.audit_logger.log_silent(&AuditEvent::repo_push_blocked(
                    &sanitized_url,
                    format!("force push blocked: {reason}"),
                ));
                return ToolCallResult::error(reason.to_string());
            }
        }

        // Execute the push with timing
        let start = Instant::now();
        let branch = args.branch.clone();
        let force = args.force;

        match handle_repo_push(args) {
            Ok(result) => {
                let duration = start.elapsed();
                self.audit_logger.log_silent(&AuditEvent::repo_push_success(
                    &sanitized_url,
                    &result.branch,
                    &result.commit,
                    force,
                    duration,
                ));

                // Return the result as JSON text
                match serde_json::to_string_pretty(&result) {
                    Ok(json) => ToolCallResult::text(json),
                    Err(e) => ToolCallResult::error(format!("Failed to serialize result: {e}")),
                }
            }
            Err(e) => {
                self.audit_logger.log_silent(&AuditEvent::repo_push_failed(
                    &sanitized_url,
                    format!("push to {branch} failed: {e}"),
                ));
                ToolCallResult::error(e.to_string())
            }
        }
    }

    /// Calls the `repo/clone_start` tool (Tier 2).
    ///
    /// Starts a chunked streaming session for a repository clone.
    fn call_repo_clone_start_tool(&self, arguments: &Value) -> ToolCallResult {
        use crate::git2_ops::auth::sanitize_url_for_logging;

        // Parse arguments
        let args: RepoCloneStartArgs = match serde_json::from_value(arguments.clone()) {
            Ok(a) => a,
            Err(e) => {
                return ToolCallResult::error(format!("Invalid arguments: {e}"));
            }
        };

        let sanitized_url = sanitize_url_for_logging(&args.url);

        // Check rate limiter
        if !self.rate_limiter.try_acquire() {
            self.audit_logger
                .log_silent(&AuditEvent::repo_clone_blocked(
                    &sanitized_url,
                    "rate limit exceeded",
                ));
            return ToolCallResult::error(
                "Rate limit exceeded. Please wait before sending more requests.",
            );
        }

        // Check repo filter
        if let Some(reason) = self
            .repo_filter
            .check("clone", std::slice::from_ref(&args.url))
            .reason()
        {
            self.audit_logger
                .log_silent(&AuditEvent::repo_clone_blocked(&sanitized_url, reason));
            return ToolCallResult::error(reason.to_string());
        }

        // Execute the clone_start
        let start = Instant::now();
        match handle_repo_clone_start(args, &self.streaming_sessions) {
            Ok(result) => {
                let duration = start.elapsed();
                self.audit_logger
                    .log_silent(&AuditEvent::repo_clone_success(
                        &sanitized_url,
                        &result.branch,
                        &result.commit,
                        0, // file_count not known until all chunks retrieved
                        result.total_size,
                        duration,
                    ));

                // Return the result as JSON text
                match serde_json::to_string_pretty(&result) {
                    Ok(json) => ToolCallResult::text(json),
                    Err(e) => ToolCallResult::error(format!("Failed to serialize result: {e}")),
                }
            }
            Err(e) => {
                self.audit_logger.log_silent(&AuditEvent::repo_clone_failed(
                    &sanitized_url,
                    e.to_string(),
                ));
                ToolCallResult::error(e.to_string())
            }
        }
    }

    /// Calls the `repo/clone_chunk` tool (Tier 2).
    ///
    /// Retrieves a chunk from a streaming session.
    fn call_repo_clone_chunk_tool(&self, arguments: &Value) -> ToolCallResult {
        // Parse arguments
        let args: RepoCloneChunkArgs = match serde_json::from_value(arguments.clone()) {
            Ok(a) => a,
            Err(e) => {
                return ToolCallResult::error(format!("Invalid arguments: {e}"));
            }
        };

        // Execute the chunk retrieval
        match handle_repo_clone_chunk(args, &self.streaming_sessions) {
            Ok(result) => {
                // Return the result as JSON text
                match serde_json::to_string_pretty(&result) {
                    Ok(json) => ToolCallResult::text(json),
                    Err(e) => ToolCallResult::error(format!("Failed to serialize result: {e}")),
                }
            }
            Err(e) => ToolCallResult::error(e.to_string()),
        }
    }

    /// Calls the `repo/clone_cancel` tool (Tier 2).
    ///
    /// Cancels a streaming session and frees resources.
    fn call_repo_clone_cancel_tool(&self, arguments: &Value) -> ToolCallResult {
        // Parse arguments
        let args: RepoCloneCancelArgs = match serde_json::from_value(arguments.clone()) {
            Ok(a) => a,
            Err(e) => {
                return ToolCallResult::error(format!("Invalid arguments: {e}"));
            }
        };

        // Execute the cancel
        match handle_repo_clone_cancel(args, &self.streaming_sessions) {
            Ok(result) => {
                // Return the result as JSON text
                match serde_json::to_string_pretty(&result) {
                    Ok(json) => ToolCallResult::text(json),
                    Err(e) => ToolCallResult::error(format!("Failed to serialize result: {e}")),
                }
            }
            Err(e) => ToolCallResult::error(e.to_string()),
        }
    }

    /// Calls the `repo/refs` tool.
    ///
    /// Lists branches and tags from a remote repository without cloning.
    fn call_repo_refs_tool(&self, arguments: &Value) -> ToolCallResult {
        use crate::git2_ops::auth::sanitize_url_for_logging;

        // Parse arguments
        let args: RepoRefsArgs = match serde_json::from_value(arguments.clone()) {
            Ok(a) => a,
            Err(e) => {
                return ToolCallResult::error(format!("Invalid arguments: {e}"));
            }
        };

        let sanitized_url = sanitize_url_for_logging(&args.url);

        // Check rate limiter
        if !self.rate_limiter.try_acquire() {
            self.audit_logger
                .log_silent(&AuditEvent::repo_clone_blocked(
                    &sanitized_url,
                    "rate limit exceeded",
                ));
            return ToolCallResult::error(
                "Rate limit exceeded. Please wait before sending more requests.",
            );
        }

        // Check repo filter
        if let Some(reason) = self
            .repo_filter
            .check("ls-remote", std::slice::from_ref(&args.url))
            .reason()
        {
            self.audit_logger
                .log_silent(&AuditEvent::repo_clone_blocked(&sanitized_url, reason));
            return ToolCallResult::error(reason.to_string());
        }

        // Execute the refs listing
        let start = Instant::now();
        match handle_repo_refs(args) {
            Ok(result) => {
                let duration = start.elapsed();
                tracing::info!(
                    url = %sanitized_url,
                    branches = result.branches.len(),
                    tags = result.tags.len(),
                    default_branch = %result.default_branch,
                    duration_ms = duration.as_millis(),
                    "repo/refs complete"
                );

                // Return the result as JSON text
                match serde_json::to_string_pretty(&result) {
                    Ok(json) => ToolCallResult::text(json),
                    Err(e) => ToolCallResult::error(format!("Failed to serialize result: {e}")),
                }
            }
            Err(e) => {
                tracing::warn!(
                    url = %sanitized_url,
                    error = %e,
                    "repo/refs failed"
                );
                ToolCallResult::error(e.to_string())
            }
        }
    }

    /// Calls the `repo/diff` tool.
    ///
    /// Generates a unified diff between two commits from a remote repository.
    fn call_repo_diff_tool(&self, arguments: &Value) -> ToolCallResult {
        use crate::git2_ops::auth::sanitize_url_for_logging;

        // Parse arguments
        let args: RepoDiffArgs = match serde_json::from_value(arguments.clone()) {
            Ok(a) => a,
            Err(e) => {
                return ToolCallResult::error(format!("Invalid arguments: {e}"));
            }
        };

        let sanitized_url = sanitize_url_for_logging(&args.url);

        // Check rate limiter
        if !self.rate_limiter.try_acquire() {
            self.audit_logger
                .log_silent(&AuditEvent::repo_clone_blocked(
                    &sanitized_url,
                    "rate limit exceeded",
                ));
            return ToolCallResult::error(
                "Rate limit exceeded. Please wait before sending more requests.",
            );
        }

        // Check repo filter
        if let Some(reason) = self
            .repo_filter
            .check("diff", std::slice::from_ref(&args.url))
            .reason()
        {
            self.audit_logger
                .log_silent(&AuditEvent::repo_clone_blocked(&sanitized_url, reason));
            return ToolCallResult::error(reason.to_string());
        }

        // Execute the diff generation
        let start = Instant::now();
        match handle_repo_diff(args) {
            Ok(result) => {
                let duration = start.elapsed();
                tracing::info!(
                    url = %sanitized_url,
                    files = result.stats.files_changed,
                    insertions = result.stats.insertions,
                    deletions = result.stats.deletions,
                    duration_ms = duration.as_millis(),
                    "repo/diff complete"
                );

                // Return the result as JSON text
                match serde_json::to_string_pretty(&result) {
                    Ok(json) => ToolCallResult::text(json),
                    Err(e) => ToolCallResult::error(format!("Failed to serialize result: {e}")),
                }
            }
            Err(e) => {
                tracing::warn!(
                    url = %sanitized_url,
                    error = %e,
                    "repo/diff failed"
                );
                ToolCallResult::error(e.to_string())
            }
        }
    }

    /// Calls the `repo/pull` tool.
    ///
    /// Fetches changes since a known commit for incremental sync.
    fn call_repo_pull_tool(&self, arguments: &Value) -> ToolCallResult {
        use crate::git2_ops::auth::sanitize_url_for_logging;

        // Parse arguments
        let args: RepoPullArgs = match serde_json::from_value(arguments.clone()) {
            Ok(a) => a,
            Err(e) => {
                return ToolCallResult::error(format!("Invalid arguments: {e}"));
            }
        };

        let sanitized_url = sanitize_url_for_logging(&args.url);

        // Check rate limiter
        if !self.rate_limiter.try_acquire() {
            self.audit_logger
                .log_silent(&AuditEvent::repo_clone_blocked(
                    &sanitized_url,
                    "rate limit exceeded",
                ));
            return ToolCallResult::error(
                "Rate limit exceeded. Please wait before sending more requests.",
            );
        }

        // Check repo filter
        if let Some(reason) = self
            .repo_filter
            .check("fetch", std::slice::from_ref(&args.url))
            .reason()
        {
            self.audit_logger
                .log_silent(&AuditEvent::repo_clone_blocked(&sanitized_url, reason));
            return ToolCallResult::error(reason.to_string());
        }

        // Execute the pull
        let start = Instant::now();
        match handle_repo_pull(args) {
            Ok(result) => {
                let duration = start.elapsed();
                if result.up_to_date {
                    tracing::info!(
                        url = %sanitized_url,
                        duration_ms = duration.as_millis(),
                        "repo/pull: already up to date"
                    );
                } else {
                    tracing::info!(
                        url = %sanitized_url,
                        commits = result.stats.commits,
                        files = result.stats.files_changed,
                        added = result.stats.files_added,
                        modified = result.stats.files_modified,
                        deleted = result.stats.files_deleted,
                        duration_ms = duration.as_millis(),
                        "repo/pull complete"
                    );
                }

                // Return the result as JSON text
                match serde_json::to_string_pretty(&result) {
                    Ok(json) => ToolCallResult::text(json),
                    Err(e) => ToolCallResult::error(format!("Failed to serialize result: {e}")),
                }
            }
            Err(e) => {
                tracing::warn!(
                    url = %sanitized_url,
                    error = %e,
                    "repo/pull failed"
                );
                ToolCallResult::error(e.to_string())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Creates a test server with minimal configuration.
    fn create_test_server() -> McpServer {
        let security_config = SecurityConfig::default();
        let git_identity = GitIdentity::default();
        let audit_logger = AuditLogger::disabled();

        McpServer::new(security_config, git_identity, audit_logger)
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
