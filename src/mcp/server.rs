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

use crate::config::{LfsConfig, ProxyConfig, SessionConfig, SubmoduleConfig};
use crate::mcp::protocol::{
    ErrorCode, IncomingMessage, JsonRpcError, JsonRpcErrorData, JsonRpcNotification,
    JsonRpcRequest, JsonRpcResponse, RequestId, MCP_PROTOCOL_VERSION, SERVER_NAME,
};
use crate::mcp::tools::{
    handle_repo_clone, handle_repo_clone_cancel, handle_repo_clone_chunk, handle_repo_clone_start,
    handle_repo_clone_status, handle_repo_diff, handle_repo_pull, handle_repo_push,
    handle_repo_refs, RepoCloneArgs, RepoCloneCancelArgs, RepoCloneChunkArgs, RepoCloneStartArgs,
    RepoCloneStatusArgs, RepoDiffArgs, RepoPullArgs, RepoPushArgs, RepoRefsArgs,
};
use crate::mcp::transport::StdioTransport;
use crate::security::{
    AuditEvent, AuditLogger, BranchGuard, PushGuard, RateLimiter, RepoFilter, SecurityGuard,
    ShutdownReason,
};
use crate::streaming::chunked::StreamingSessionManager;
use crate::util::sanitize_for_log;

/// Server state in the MCP lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerState {
    /// Waiting for `initialize` request.
    AwaitingInit,
    /// `initialize` received, waiting for `notifications/initialized`.
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

/// Parameters for the `initialize` request.
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
/// This identity is communicated to the AI during initialisation so they
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
    /// Proxy configuration for network connections.
    proxy_config: ProxyConfig,
    /// Git LFS configuration (retry behaviour, size limits).
    lfs_config: LfsConfig,
    /// Submodule configuration (depth, filtering, failure limits).
    submodule_config: SubmoduleConfig,
}

impl McpServer {
    /// Creates a new MCP server with the given dependencies.
    ///
    /// # Arguments
    ///
    /// * `security_config` — Security settings from configuration
    /// * `git_identity` — Git identity for AI-assisted commits
    /// * `audit_logger` — Audit logger for recording operations
    /// * `proxy_config` — Proxy configuration for network connections
    /// * `session_config` — Session management settings
    /// * `lfs_config` — Git LFS configuration (retry behaviour, size limits)
    /// * `submodule_config` — Submodule configuration (depth, filtering, failure limits)
    #[must_use]
    pub fn new(
        security_config: SecurityConfig,
        git_identity: GitIdentity,
        audit_logger: AuditLogger,
        proxy_config: ProxyConfig,
        session_config: &SessionConfig,
        lfs_config: LfsConfig,
        submodule_config: SubmoduleConfig,
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
            streaming_sessions: StreamingSessionManager::new(
                session_config.timeout(),
                session_config.max_streaming_sessions,
            ),
            git_identity,
            proxy_config,
            lfs_config,
            submodule_config,
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
            return;
        }
        // All other notifications (including unknown ones) are ignored per
        // JSON-RPC spec, but trace them at debug level so an operator
        // diagnosing protocol mismatches can see which methods were sent.
        tracing::debug!(
            method = %notif.method,
            state = ?self.state,
            "ignoring notification (unknown method or wrong state)"
        );
    }

    /// Handles the `initialize` request.
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
        let params: InitializeParams = req
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

        // Sanitise the client-controlled values we're about to log:
        // escape control chars / ANSI escapes (so a buggy or hostile
        // client can't disrupt log readers) and cap the length (so a
        // 1-MiB name can't flood the log file).
        let safe_proto = sanitize_for_log(&params.protocol_version);

        // Log the connecting client's identity for diagnostic visibility.
        // The MCP spec says client_info is optional, so handle the absent
        // case gracefully. Single-line macro form so cargo-llvm-cov
        // counts the call site as covered even when the test runner
        // doesn't enable INFO-level logging (`tracing` doesn't evaluate
        // macro args when the level is below threshold).
        if let Some(client_info) = params.client_info.as_ref() {
            let safe_name = sanitize_for_log(&client_info.name);
            let safe_version =
                sanitize_for_log(client_info.version.as_deref().unwrap_or("(unspecified)"));
            tracing::info!(client_name = %safe_name, client_version = %safe_version, client_protocol_version = %safe_proto, "client connected");
        } else {
            tracing::info!(client_protocol_version = %safe_proto, "client connected (no clientInfo)");
        }

        // Protocol version: per the MCP spec, we MUST respond with a
        // version we support. We currently only support one
        // (`MCP_PROTOCOL_VERSION`), so we always return that. If the
        // client requested a different version, log a warning so the
        // operator can see the mismatch — the client may still proceed
        // (the spec says it can reconcile by checking the response) or
        // disconnect (if it strictly requires its requested version).
        if params.protocol_version != MCP_PROTOCOL_VERSION {
            tracing::warn!(
                requested = %safe_proto,
                supported = MCP_PROTOCOL_VERSION,
                "client requested unsupported MCP protocol version; \
                 responding with our supported version"
            );
        }
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
            "repo_clone" => self.call_repo_clone_tool(&params.arguments),
            "repo_push" => self.call_repo_push_tool(&params.arguments),
            "repo_refs" => self.call_repo_refs_tool(&params.arguments),
            "repo_diff" => self.call_repo_diff_tool(&params.arguments),
            "repo_pull" => self.call_repo_pull_tool(&params.arguments),
            // Tier 2: Chunked streaming tools
            "repo_clone_start" => self.call_repo_clone_start_tool(&params.arguments),
            "repo_clone_chunk" => self.call_repo_clone_chunk_tool(&params.arguments),
            "repo_clone_cancel" => self.call_repo_clone_cancel_tool(&params.arguments),
            "repo_clone_status" => self.call_repo_clone_status_tool(&params.arguments),
            // Utility tools
            "helper_script" => Self::call_helper_script_tool(),
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
            // IMPORTANT: Helper script should be first so AI discovers it
            ToolDefinition {
                name: "helper_script".to_string(),
                description: Some(
                    "**CALL THIS FIRST** before using repo_clone, repo_pull, or repo_push. \
                     Returns a Python helper script that handles JSON parsing, base64 decoding, \
                     and archive extraction automatically. Save it as git_proxy_helper.py and use: \
                     'python git_proxy_helper.py extract <result.json> <dir>' for clone/pull, \
                     'python git_proxy_helper.py bundle <repo> <commit>' for push."
                        .to_string(),
                ),
                input_schema: json!({
                    "type": "object",
                    "properties": {},
                    "required": []
                }),
            },
            // Tier 1: Stream repository as tar.gz
            ToolDefinition {
                name: "repo_clone".to_string(),
                description: Some(
                    "Clone a repository and return it as a base64-encoded tar.gz archive. \
                     **Tip:** Use helper_script first to get a Python script that handles \
                     extraction automatically. The repository is fetched using your local Git \
                     credentials (SSH agent or credential helpers) but no source files are \
                     written to your disk."
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
                            "description": "Branch to clone. Omit to use the remote's default branch (typically 'main' or 'master')."
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
                        },
                        "submodule_depth": {
                            "type": "integer",
                            "description": "Maximum submodule recursion depth. Omit for unlimited (git default). 1 = top-level only, 0 = skip submodules."
                        },
                        "submodule_include": {
                            "type": "array",
                            "items": {"type": "string"},
                            "description": "Glob patterns for submodule paths to include. Only submodules matching at least one pattern are fetched."
                        },
                        "submodule_exclude": {
                            "type": "array",
                            "items": {"type": "string"},
                            "description": "Glob patterns for submodule paths to exclude. Exclusions take precedence over inclusions."
                        }
                    },
                    "required": ["url"]
                }),
            },
            // Tier 1: Push a git bundle to remote
            ToolDefinition {
                name: "repo_push".to_string(),
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
                name: "repo_clone_start".to_string(),
                description: Some(
                    "Start a chunked clone for large repositories. Returns a session ID that \
                     can be used with repo_clone_chunk to retrieve the data in pieces. \
                     Use this instead of repo_clone when working with large repositories \
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
                            "description": "Branch to clone. Omit to use the remote's default branch (typically 'main' or 'master')."
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
                            "description": "Chunk size in bytes (default: 1 MiB, range: 1 KiB – 4 MiB after clamping)"
                        },
                        "exclude_binary": {
                            "type": "boolean",
                            "description": "Exclude binary files (files with null bytes or mostly non-printable chars)."
                        },
                        "max_file_size": {
                            "type": "integer",
                            "description": "Maximum file size in bytes. Files larger than this are skipped."
                        },
                        "resolve_lfs": {
                            "type": "boolean",
                            "description": "Resolve Git LFS pointers to actual content."
                        },
                        "include_submodules": {
                            "type": "boolean",
                            "description": "Include submodule contents in the archive."
                        },
                        "submodule_depth": {
                            "type": "integer",
                            "description": "Maximum submodule recursion depth. Omit for unlimited (git default). 1 = top-level only, 0 = skip submodules."
                        },
                        "submodule_include": {
                            "type": "array",
                            "items": {"type": "string"},
                            "description": "Glob patterns for submodule paths to include."
                        },
                        "submodule_exclude": {
                            "type": "array",
                            "items": {"type": "string"},
                            "description": "Glob patterns for submodule paths to exclude. Exclusions take precedence over inclusions."
                        }
                    },
                    "required": ["url"]
                }),
            },
            // Tier 2: Get a chunk from streaming session
            ToolDefinition {
                name: "repo_clone_chunk".to_string(),
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
                            "description": "Session ID from repo_clone_start"
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
                name: "repo_clone_cancel".to_string(),
                description: Some(
                    "Cancel a streaming clone session and free resources. Call this if \
                     you no longer need the remaining chunks. Sessions also auto-expire \
                     after a configured period of inactivity."
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
            // Tier 2: Check status of a streaming session (resume support)
            ToolDefinition {
                name: "repo_clone_status".to_string(),
                description: Some(
                    "Check the status of a chunked clone session. Returns progress and which \
                     chunks have been delivered, enabling resume after interruption."
                        .to_string(),
                ),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "session_id": {
                            "type": "string",
                            "description": "Session ID from repo_clone_start"
                        }
                    },
                    "required": ["session_id"]
                }),
            },
            // List remote refs without cloning
            ToolDefinition {
                name: "repo_refs".to_string(),
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
                name: "repo_diff".to_string(),
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
                name: "repo_pull".to_string(),
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

    /// Calls the `repo_clone` tool.
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
        match handle_repo_clone(
            args,
            &self.proxy_config,
            &self.lfs_config,
            &self.submodule_config,
        ) {
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

    /// Calls the `repo_push` tool.
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

        // Check branch guard (block force pushes to protected branches). The
        // server knows the branch and force flag directly, so it uses the
        // structured check rather than the CLI-arg form (which can't see the
        // force flag from a lone branch name).
        if let Some(reason) = self
            .branch_guard
            .check_force_push(&args.branch, args.force)
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

        match handle_repo_push(args, &self.proxy_config) {
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

    /// Calls the `repo_clone_start` tool (Tier 2).
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
        match handle_repo_clone_start(
            args,
            &self.proxy_config,
            &self.lfs_config,
            &self.submodule_config,
            &self.streaming_sessions,
        ) {
            Ok(result) => {
                let duration = start.elapsed();
                self.audit_logger
                    .log_silent(&AuditEvent::repo_clone_success(
                        &sanitized_url,
                        &result.branch,
                        &result.commit,
                        // `RepoCloneStartResult` carries `file_count` as soon
                        // as the archive is built (which is before any chunks
                        // are retrieved) — log it here rather than the
                        // hard-coded 0 the previous comment claimed was
                        // unavoidable.
                        result.file_count,
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

    /// Calls the `repo_clone_chunk` tool (Tier 2).
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

    /// Calls the `repo_clone_cancel` tool (Tier 2).
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

    /// Calls the `repo_clone_status` tool (Tier 2).
    ///
    /// Returns resume information for a streaming session.
    fn call_repo_clone_status_tool(&self, arguments: &Value) -> ToolCallResult {
        // Parse arguments
        let args: RepoCloneStatusArgs = match serde_json::from_value(arguments.clone()) {
            Ok(a) => a,
            Err(e) => {
                return ToolCallResult::error(format!("Invalid arguments: {e}"));
            }
        };

        // Execute the status check
        match handle_repo_clone_status(args, &self.streaming_sessions) {
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

    /// Calls the `repo_refs` tool.
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
            self.audit_logger.log_silent(&AuditEvent::repo_refs_blocked(
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
                .log_silent(&AuditEvent::repo_refs_blocked(&sanitized_url, reason));
            return ToolCallResult::error(reason.to_string());
        }

        // Execute the refs listing
        let start = Instant::now();
        match handle_repo_refs(args, &self.proxy_config) {
            Ok(result) => {
                let duration = start.elapsed();
                tracing::info!(
                    url = %sanitized_url,
                    branches = result.branches.len(),
                    tags = result.tags.len(),
                    default_branch = %result.default_branch,
                    duration_ms = duration.as_millis(),
                    "repo_refs complete"
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
                    "repo_refs failed"
                );
                ToolCallResult::error(e.to_string())
            }
        }
    }

    /// Calls the `repo_diff` tool.
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
            self.audit_logger.log_silent(&AuditEvent::repo_diff_blocked(
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
                .log_silent(&AuditEvent::repo_diff_blocked(&sanitized_url, reason));
            return ToolCallResult::error(reason.to_string());
        }

        // Execute the diff generation
        let start = Instant::now();
        match handle_repo_diff(args, &self.proxy_config) {
            Ok(result) => {
                let duration = start.elapsed();
                tracing::info!(
                    url = %sanitized_url,
                    files = result.stats.files_changed,
                    insertions = result.stats.insertions,
                    deletions = result.stats.deletions,
                    duration_ms = duration.as_millis(),
                    "repo_diff complete"
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
                    "repo_diff failed"
                );
                ToolCallResult::error(e.to_string())
            }
        }
    }

    /// Calls the `repo_pull` tool.
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
            self.audit_logger.log_silent(&AuditEvent::repo_pull_blocked(
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
                .log_silent(&AuditEvent::repo_pull_blocked(&sanitized_url, reason));
            return ToolCallResult::error(reason.to_string());
        }

        // Execute the pull
        let start = Instant::now();
        match handle_repo_pull(args, &self.proxy_config) {
            Ok(result) => {
                let duration = start.elapsed();
                if result.up_to_date {
                    tracing::info!(
                        url = %sanitized_url,
                        duration_ms = duration.as_millis(),
                        "repo_pull: already up to date"
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
                        "repo_pull complete"
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
                    "repo_pull failed"
                );
                ToolCallResult::error(e.to_string())
            }
        }
    }

    /// Calls the `helper_script` tool.
    ///
    /// This tool returns a Python helper script that simplifies working with
    /// git-proxy-mcp responses. No arguments required.
    fn call_helper_script_tool() -> ToolCallResult {
        use crate::mcp::tools::handle_helper_script;

        let result = handle_helper_script();

        tracing::info!("helper_script tool called");

        match serde_json::to_string_pretty(&result) {
            Ok(json) => ToolCallResult::text(json),
            Err(e) => ToolCallResult::error(format!("Failed to serialize result: {e}")),
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

        McpServer::new(
            security_config,
            git_identity,
            audit_logger,
            ProxyConfig::default(),
            &SessionConfig::default(),
            LfsConfig::default(),
            SubmoduleConfig::default(),
        )
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

    fn make_request(id: i64, method: &str, params: Option<Value>) -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: RequestId::Number(id),
            method: method.to_string(),
            params,
        }
    }

    fn initialise_server(server: &mut McpServer) {
        let init_req = make_request(
            1,
            "initialize",
            Some(json!({
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "test-client", "version": "1.0"},
            })),
        );
        let _ = server.handle_initialize(&init_req).unwrap();
        // Send initialized notification to move to Running state
        let notif = JsonRpcNotification {
            jsonrpc: "2.0".to_string(),
            method: "notifications/initialized".to_string(),
            params: None,
        };
        server.handle_notification(&notif);
    }

    #[test]
    fn handle_initialize_with_valid_params_transitions_state() {
        let mut server = create_test_server();
        let req = make_request(
            1,
            "initialize",
            Some(json!({
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "test-client", "version": "1.0"},
            })),
        );
        let resp = server.handle_initialize(&req).unwrap();
        assert_eq!(server.state(), ServerState::Initialising);
        let result = &resp.result;
        assert_eq!(result["protocolVersion"], MCP_PROTOCOL_VERSION);
        assert!(result["capabilities"].is_object());
        assert!(result["serverInfo"].is_object());
    }

    #[test]
    fn handle_initialize_twice_returns_error() {
        let mut server = create_test_server();
        let req = make_request(
            1,
            "initialize",
            Some(json!({
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "test-client", "version": "1.0"},
            })),
        );
        let _ = server.handle_initialize(&req).unwrap();
        let err = server.handle_initialize(&req).unwrap_err();
        assert_eq!(err.error.code, ErrorCode::InvalidRequest.code());
        assert!(err.error.message.contains("already"));
    }

    #[test]
    fn handle_initialize_missing_params_returns_error() {
        let mut server = create_test_server();
        let req = make_request(1, "initialize", None);
        let err = server.handle_initialize(&req).unwrap_err();
        assert_eq!(err.error.code, ErrorCode::InvalidParams.code());
    }

    #[test]
    fn handle_initialize_malformed_params_returns_error() {
        let mut server = create_test_server();
        let req = make_request(1, "initialize", Some(json!("not an object")));
        let err = server.handle_initialize(&req).unwrap_err();
        assert_eq!(err.error.code, ErrorCode::InvalidParams.code());
    }

    #[test]
    fn handle_initialize_accepts_mismatched_protocol_version() {
        // The spec says we MUST respond with a version we support. A
        // client requesting an unsupported version still gets a
        // successful response (with our version), accompanied by a
        // warning log so the operator can see the mismatch. The state
        // still advances to Initialising.
        let mut server = create_test_server();
        let req = make_request(
            1,
            "initialize",
            Some(json!({
                "protocolVersion": "2099-01-01",
                "capabilities": {},
                "clientInfo": {"name": "test", "version": "1"},
            })),
        );
        let resp = server.handle_initialize(&req).unwrap();
        // We always respond with our version, regardless of what the
        // client requested.
        assert_eq!(resp.result["protocolVersion"], MCP_PROTOCOL_VERSION);
        assert_eq!(server.state(), ServerState::Initialising);
    }

    #[test]
    fn handle_initialize_accepts_missing_client_info() {
        // `clientInfo` is optional per the spec — initialize must succeed
        // and log via the "no clientInfo" path.
        let mut server = create_test_server();
        let req = make_request(
            1,
            "initialize",
            Some(json!({
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": {},
            })),
        );
        let resp = server.handle_initialize(&req).unwrap();
        assert_eq!(resp.result["protocolVersion"], MCP_PROTOCOL_VERSION);
        assert_eq!(server.state(), ServerState::Initialising);
    }

    #[test]
    fn handle_initialize_includes_git_identity_when_set() {
        let security_config = SecurityConfig::default();
        let git_identity = GitIdentity {
            name: Some("AI Assistant".to_string()),
            email: Some("ai@example.com".to_string()),
        };
        let audit_logger = AuditLogger::disabled();
        let mut server = McpServer::new(
            security_config,
            git_identity,
            audit_logger,
            ProxyConfig::default(),
            &SessionConfig::default(),
            LfsConfig::default(),
            SubmoduleConfig::default(),
        );
        let req = make_request(
            1,
            "initialize",
            Some(json!({
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "c", "version": "1"},
            })),
        );
        let resp = server.handle_initialize(&req).unwrap();
        assert!(resp.result.get("gitIdentity").is_some());
    }

    #[test]
    fn handle_tools_list_before_init_returns_error() {
        let server = create_test_server();
        let req = make_request(1, "tools/list", None);
        let err = server.handle_tools_list(&req).unwrap_err();
        assert_eq!(err.error.code, ErrorCode::InvalidRequest.code());
    }

    #[test]
    fn handle_tools_list_after_init_returns_all_tools() {
        let mut server = create_test_server();
        initialise_server(&mut server);
        let req = make_request(1, "tools/list", None);
        let resp = server.handle_tools_list(&req).unwrap();
        let tools = resp.result.get("tools").unwrap().as_array().unwrap();
        // 10 tools total: helper_script + 5 tier1 + 4 tier2
        assert!(tools.len() >= 10);
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"helper_script"));
        assert!(names.contains(&"repo_clone"));
        assert!(names.contains(&"repo_push"));
        assert!(names.contains(&"repo_pull"));
        assert!(names.contains(&"repo_diff"));
        assert!(names.contains(&"repo_refs"));
        assert!(names.contains(&"repo_clone_start"));
        assert!(names.contains(&"repo_clone_chunk"));
        assert!(names.contains(&"repo_clone_cancel"));
        assert!(names.contains(&"repo_clone_status"));
    }

    #[test]
    fn handle_tools_call_before_init_returns_error() {
        let server = create_test_server();
        let req = make_request(
            1,
            "tools/call",
            Some(json!({"name": "helper_script", "arguments": {}})),
        );
        let err = server.handle_tools_call(&req).unwrap_err();
        assert_eq!(err.error.code, ErrorCode::InvalidRequest.code());
    }

    #[test]
    fn handle_tools_call_with_missing_params_returns_error() {
        let mut server = create_test_server();
        initialise_server(&mut server);
        let req = make_request(1, "tools/call", None);
        let err = server.handle_tools_call(&req).unwrap_err();
        assert_eq!(err.error.code, ErrorCode::InvalidParams.code());
    }

    #[test]
    fn handle_tools_call_with_malformed_params_returns_error() {
        let mut server = create_test_server();
        initialise_server(&mut server);
        let req = make_request(1, "tools/call", Some(json!("not an object")));
        let err = server.handle_tools_call(&req).unwrap_err();
        assert_eq!(err.error.code, ErrorCode::InvalidParams.code());
    }

    #[test]
    fn handle_tools_call_helper_script_returns_python_script() {
        let mut server = create_test_server();
        initialise_server(&mut server);
        let req = make_request(
            1,
            "tools/call",
            Some(json!({"name": "helper_script", "arguments": {}})),
        );
        let resp = server.handle_tools_call(&req).unwrap();
        let result = &resp.result;
        // isError is skipped when false (serde skip_serializing_if)
        assert!(result.get("isError").is_none() || result["isError"] == false);
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("script"));
    }

    #[test]
    fn handle_tools_call_unknown_tool_returns_error_result() {
        let mut server = create_test_server();
        initialise_server(&mut server);
        let req = make_request(
            1,
            "tools/call",
            Some(json!({"name": "nonexistent_tool", "arguments": {}})),
        );
        // Returns Ok with isError=true (not a JSON-RPC error)
        let resp = server.handle_tools_call(&req).unwrap();
        assert_eq!(resp.result["isError"], true);
        let text = resp.result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("Unknown tool"));
    }

    #[test]
    fn handle_ping_returns_empty_object() {
        let req = make_request(42, "ping", None);
        let resp = McpServer::handle_ping(&req);
        assert_eq!(resp.id, RequestId::Number(42));
        assert!(resp.result.is_object());
    }

    #[test]
    fn handle_ping_works_in_any_state() {
        // Ping should work even before initialize
        let server = create_test_server();
        assert_eq!(server.state(), ServerState::AwaitingInit);
        let req = make_request(1, "ping", None);
        let resp = McpServer::handle_ping(&req);
        assert!(resp.result.is_object());
    }

    #[test]
    fn handle_notification_initialized_transitions_to_running() {
        let mut server = create_test_server();
        // Move to Initialising first
        let init_req = make_request(
            1,
            "initialize",
            Some(json!({
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "c", "version": "1"},
            })),
        );
        server.handle_initialize(&init_req).unwrap();
        assert_eq!(server.state(), ServerState::Initialising);

        let notif = JsonRpcNotification {
            jsonrpc: "2.0".to_string(),
            method: "notifications/initialized".to_string(),
            params: None,
        };
        server.handle_notification(&notif);
        assert_eq!(server.state(), ServerState::Running);
    }

    #[test]
    fn handle_notification_initialized_ignored_in_wrong_state() {
        let mut server = create_test_server();
        // Server is in AwaitingInit
        let notif = JsonRpcNotification {
            jsonrpc: "2.0".to_string(),
            method: "notifications/initialized".to_string(),
            params: None,
        };
        server.handle_notification(&notif);
        // State unchanged
        assert_eq!(server.state(), ServerState::AwaitingInit);
    }

    #[test]
    fn handle_notification_initialized_in_running_state_is_ignored() {
        // Sending `notifications/initialized` again after the server is
        // already in `Running` (e.g. a duplicate or out-of-order notif
        // from a buggy client) must be a no-op state-wise. The method
        // matches but the state guard fails, so the new debug! trace
        // for "ignoring notification" fires and state stays `Running`.
        // Pins the combinatorial case (right method, wrong state).
        let mut server = create_test_server();
        initialise_server(&mut server);
        assert_eq!(server.state(), ServerState::Running);

        let notif = JsonRpcNotification {
            jsonrpc: "2.0".to_string(),
            method: "notifications/initialized".to_string(),
            params: None,
        };
        server.handle_notification(&notif);
        assert_eq!(server.state(), ServerState::Running);
    }

    #[test]
    fn handle_notification_unknown_method_ignored() {
        let mut server = create_test_server();
        initialise_server(&mut server);
        let notif = JsonRpcNotification {
            jsonrpc: "2.0".to_string(),
            method: "unknown/method".to_string(),
            params: None,
        };
        server.handle_notification(&notif);
        // State unchanged
        assert_eq!(server.state(), ServerState::Running);
    }

    #[test]
    fn require_running_in_running_state_succeeds() {
        let mut server = create_test_server();
        initialise_server(&mut server);
        assert!(server.require_running(&RequestId::Number(1)).is_ok());
    }

    #[test]
    fn require_running_in_awaiting_init_fails() {
        let server = create_test_server();
        let err = server.require_running(&RequestId::Number(1)).unwrap_err();
        assert_eq!(err.error.code, ErrorCode::InvalidRequest.code());
    }

    #[test]
    fn require_running_in_initialising_fails() {
        let mut server = create_test_server();
        let init_req = make_request(
            1,
            "initialize",
            Some(json!({
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "c", "version": "1"},
            })),
        );
        server.handle_initialize(&init_req).unwrap();
        // Now in Initialising (haven't sent notifications/initialized)
        let err = server.require_running(&RequestId::Number(1)).unwrap_err();
        assert_eq!(err.error.code, ErrorCode::InvalidRequest.code());
    }

    #[test]
    fn call_repo_clone_tool_with_invalid_args_returns_error() {
        let mut server = create_test_server();
        initialise_server(&mut server);
        // Missing required url field
        let result = server.call_repo_clone_tool(&json!({}));
        assert!(result.is_error);
    }

    #[test]
    fn call_repo_push_tool_with_invalid_args_returns_error() {
        let mut server = create_test_server();
        initialise_server(&mut server);
        let result = server.call_repo_push_tool(&json!({}));
        assert!(result.is_error);
    }

    #[test]
    fn call_repo_refs_tool_with_invalid_args_returns_error() {
        let mut server = create_test_server();
        initialise_server(&mut server);
        let result = server.call_repo_refs_tool(&json!({}));
        assert!(result.is_error);
    }

    #[test]
    fn call_repo_diff_tool_with_invalid_args_returns_error() {
        let mut server = create_test_server();
        initialise_server(&mut server);
        let result = server.call_repo_diff_tool(&json!({}));
        assert!(result.is_error);
    }

    #[test]
    fn call_repo_pull_tool_with_invalid_args_returns_error() {
        let mut server = create_test_server();
        initialise_server(&mut server);
        let result = server.call_repo_pull_tool(&json!({}));
        assert!(result.is_error);
    }

    #[test]
    fn call_repo_clone_chunk_tool_with_invalid_args_returns_error() {
        let mut server = create_test_server();
        initialise_server(&mut server);
        let result = server.call_repo_clone_chunk_tool(&json!({}));
        assert!(result.is_error);
    }

    #[test]
    fn call_repo_clone_cancel_tool_with_invalid_args_returns_error() {
        let mut server = create_test_server();
        initialise_server(&mut server);
        let result = server.call_repo_clone_cancel_tool(&json!({}));
        assert!(result.is_error);
    }

    #[test]
    fn call_repo_clone_status_tool_with_invalid_args_returns_error() {
        let mut server = create_test_server();
        initialise_server(&mut server);
        let result = server.call_repo_clone_status_tool(&json!({}));
        assert!(result.is_error);
    }

    #[test]
    fn call_repo_clone_status_tool_with_unknown_session() {
        let mut server = create_test_server();
        initialise_server(&mut server);
        let result = server.call_repo_clone_status_tool(&json!({
            "session_id": "nonexistent_session"
        }));
        assert!(result.is_error);
    }

    #[test]
    fn call_helper_script_tool_returns_text_content() {
        let result = McpServer::call_helper_script_tool();
        assert!(!result.is_error);
        assert_eq!(result.content.len(), 1);
    }

    #[test]
    fn call_repo_diff_tool_with_blocked_url_returns_error() {
        let security_config = SecurityConfig {
            repo_blocklist: Some(vec!["github.com/blocked/*".to_string()]),
            ..Default::default()
        };
        let mut server = McpServer::new(
            security_config,
            GitIdentity::default(),
            AuditLogger::disabled(),
            ProxyConfig::default(),
            &SessionConfig::default(),
            LfsConfig::default(),
            SubmoduleConfig::default(),
        );
        initialise_server(&mut server);
        let result = server.call_repo_diff_tool(&json!({
            "url": "https://github.com/blocked/repo.git",
            "base_commit": "abc",
            "head_commit": "def",
        }));
        assert!(result.is_error);
    }

    #[test]
    fn call_repo_pull_tool_with_blocked_url_returns_error() {
        let security_config = SecurityConfig {
            repo_blocklist: Some(vec!["github.com/blocked/*".to_string()]),
            ..Default::default()
        };
        let mut server = McpServer::new(
            security_config,
            GitIdentity::default(),
            AuditLogger::disabled(),
            ProxyConfig::default(),
            &SessionConfig::default(),
            LfsConfig::default(),
            SubmoduleConfig::default(),
        );
        initialise_server(&mut server);
        let result = server.call_repo_pull_tool(&json!({
            "url": "https://github.com/blocked/repo.git",
            "branch": "main",
            "since_commit": "abc123",
        }));
        assert!(result.is_error);
    }

    #[test]
    fn rate_limit_exhaustion_blocks_subsequent_calls() {
        // Configure a tiny rate limit (max_burst=1, refill=0) so the
        // first call exhausts the bucket and subsequent calls are
        // rejected with the rate-limit error message. This exercises
        // the rate-limit branch in tool dispatch (refs/diff/pull all
        // share the same shape).
        let security_config = SecurityConfig {
            rate_limit_max_burst: 1,
            rate_limit_refill_rate: 0.0,
            ..Default::default()
        };
        let mut server = McpServer::new(
            security_config,
            GitIdentity::default(),
            AuditLogger::disabled(),
            ProxyConfig::default(),
            &SessionConfig::default(),
            LfsConfig::default(),
            SubmoduleConfig::default(),
        );
        initialise_server(&mut server);
        // First call: consumes the bucket's only token.
        //
        // The dispatch order in every `call_repo_*_tool` is: parse args
        // → sanitise URL → `try_acquire()` → repo-filter check →
        // execute. The token is consumed in `try_acquire()` BEFORE
        // any actual network operation, so even when the underlying
        // `handle_repo_refs` fails, the bucket has already been
        // drained.
        //
        // We use `.invalid` (RFC 6761 reserved TLD — guaranteed never
        // to resolve in DNS) so the failed network attempt completes
        // in milliseconds rather than waiting for a TCP timeout. CI
        // runners and developer machines all fail-fast here. (Same
        // pattern as the auth.rs e2e test from PR #153.)
        //
        // We deliberately don't assert on the first call's outcome —
        // it returns an error, but the test only cares that the token
        // got consumed.
        let _ = server.call_repo_refs_tool(&json!({
            "url": "https://nonexistent-rate-limit-test.invalid/repo.git",
        }));
        // Second call: bucket empty, must be blocked.
        let result = server.call_repo_diff_tool(&json!({
            "url": "https://nonexistent-rate-limit-test.invalid/repo.git",
            "base_commit": "a",
            "head_commit": "b",
        }));
        assert!(result.is_error);
        // `ToolContent::Text` is the only variant — irrefutable pattern.
        let ToolContent::Text { text } = &result.content[0];
        assert!(
            text.contains("Rate limit"),
            "expected rate-limit error, got: {text}"
        );
        // Third call (pull): same bucket, still blocked.
        let result = server.call_repo_pull_tool(&json!({
            "url": "https://nonexistent-rate-limit-test.invalid/repo.git",
            "branch": "main",
            "since_commit": "abc",
        }));
        assert!(result.is_error);
    }

    #[test]
    fn call_repo_refs_tool_with_blocked_url_returns_error() {
        let security_config = SecurityConfig {
            repo_blocklist: Some(vec!["github.com/blocked/*".to_string()]),
            ..Default::default()
        };
        let git_identity = GitIdentity::default();
        let audit_logger = AuditLogger::disabled();
        let mut server = McpServer::new(
            security_config,
            git_identity,
            audit_logger,
            ProxyConfig::default(),
            &SessionConfig::default(),
            LfsConfig::default(),
            SubmoduleConfig::default(),
        );
        initialise_server(&mut server);
        let result = server.call_repo_refs_tool(&json!({
            "url": "https://github.com/blocked/repo.git"
        }));
        assert!(result.is_error);
    }

    #[test]
    fn tool_call_result_text_creation() {
        let result = ToolCallResult::text("hello");
        assert!(!result.is_error);
        match &result.content[0] {
            ToolContent::Text { text } => assert_eq!(text, "hello"),
        }
    }

    #[test]
    fn tool_call_result_serialises_with_fields() {
        let result = ToolCallResult::text("ok");
        let json = serde_json::to_value(&result).unwrap();
        // is_error is skipped when false
        assert!(json.get("isError").is_none());
        assert!(json["content"].is_array());
    }

    #[test]
    fn tool_call_result_error_serialises_correctly() {
        let result = ToolCallResult::error("boom");
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["isError"], true);
    }

    #[test]
    fn server_capabilities_default_has_tools() {
        let caps = ServerCapabilities::default();
        let json = serde_json::to_value(&caps).unwrap();
        assert!(json["tools"].is_object());
    }

    #[test]
    fn server_info_has_correct_name_and_version() {
        let info = ServerInfo::default();
        assert_eq!(info.name, "git-proxy-mcp");
        // Version should match Cargo.toml package version
        assert!(!info.version.is_empty());
        assert!(info.version.chars().any(|c| c.is_ascii_digit()));
    }

    #[test]
    fn git_identity_is_partial_when_name_set() {
        let id = GitIdentity {
            name: Some("foo".into()),
            email: None,
        };
        assert!(id.is_partial());
    }

    #[test]
    fn git_identity_is_partial_when_email_set() {
        let id = GitIdentity {
            name: None,
            email: Some("e@x".into()),
        };
        assert!(id.is_partial());
    }

    #[test]
    fn git_identity_is_not_partial_when_empty() {
        let id = GitIdentity::default();
        assert!(!id.is_partial());
    }

    #[test]
    fn git_identity_is_partial_when_both_set() {
        let id = GitIdentity {
            name: Some("foo".into()),
            email: Some("e@x".into()),
        };
        assert!(id.is_partial());
    }
}
