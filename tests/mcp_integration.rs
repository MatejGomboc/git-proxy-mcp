//! Integration tests for MCP protocol handling.
//!
//! These tests verify the MCP server's JSON-RPC 2.0 protocol implementation,
//! including request/response handling, error responses, and lifecycle management.

use git_proxy_mcp::mcp::protocol::{
    parse_message, IncomingMessage, JsonRpcError, JsonRpcResponse, RequestId,
};
use git_proxy_mcp::mcp::server::{McpServer, ServerState};

// =============================================================================
// Protocol Parsing Tests
// =============================================================================

#[test]
fn test_parse_initialize_request() {
    let json = r#"{
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "test-client",
                "version": "1.0.0"
            }
        }
    }"#;

    let result = parse_message(json);
    assert!(result.is_ok());

    if let IncomingMessage::Request(req) = result.unwrap() {
        assert_eq!(req.method, "initialize");
        assert_eq!(req.id, RequestId::Number(1));
    } else {
        panic!("Expected Request");
    }
}

#[test]
fn test_parse_tools_list_request() {
    let json = r#"{
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/list"
    }"#;

    let result = parse_message(json);
    assert!(result.is_ok());

    if let IncomingMessage::Request(req) = result.unwrap() {
        assert_eq!(req.method, "tools/list");
    } else {
        panic!("Expected Request");
    }
}

#[test]
fn test_parse_tools_call_request() {
    let json = r#"{
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {
            "name": "git",
            "arguments": {
                "command": "status",
                "args": []
            }
        }
    }"#;

    let result = parse_message(json);
    assert!(result.is_ok());

    if let IncomingMessage::Request(req) = result.unwrap() {
        assert_eq!(req.method, "tools/call");
        assert!(req.params.is_some());
    } else {
        panic!("Expected Request");
    }
}

#[test]
fn test_parse_notification() {
    let json = r#"{
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    }"#;

    let result = parse_message(json);
    assert!(result.is_ok());

    if let IncomingMessage::Notification(notif) = result.unwrap() {
        assert_eq!(notif.method, "notifications/initialized");
    } else {
        panic!("Expected Notification");
    }
}

#[test]
fn test_parse_invalid_json() {
    let json = "not valid json";
    let result = parse_message(json);
    assert!(result.is_err());

    let error = result.unwrap_err();
    assert_eq!(error.error.code, -32700); // Parse error
}

#[test]
fn test_parse_missing_jsonrpc_version() {
    let json = r#"{
        "id": 1,
        "method": "test"
    }"#;

    let result = parse_message(json);
    assert!(result.is_err());

    let error = result.unwrap_err();
    assert_eq!(error.error.code, -32600); // Invalid request
}

#[test]
fn test_parse_wrong_jsonrpc_version() {
    let json = r#"{
        "jsonrpc": "1.0",
        "id": 1,
        "method": "test"
    }"#;

    let result = parse_message(json);
    assert!(result.is_err());

    let error = result.unwrap_err();
    assert_eq!(error.error.code, -32600); // Invalid request
}

#[test]
fn test_parse_string_id() {
    let json = r#"{
        "jsonrpc": "2.0",
        "id": "request-123",
        "method": "test"
    }"#;

    let result = parse_message(json);
    assert!(result.is_ok());

    if let IncomingMessage::Request(req) = result.unwrap() {
        assert_eq!(req.id, RequestId::String("request-123".to_string()));
    } else {
        panic!("Expected Request");
    }
}

// =============================================================================
// Response Serialization Tests
// =============================================================================

#[test]
fn test_success_response_serialization() {
    let response =
        JsonRpcResponse::success(RequestId::Number(1), serde_json::json!({"status": "ok"}));

    let json = serde_json::to_string(&response).expect("Serialization should succeed");

    assert!(json.contains("\"jsonrpc\":\"2.0\""));
    assert!(json.contains("\"id\":1"));
    assert!(json.contains("\"result\""));
    assert!(json.contains("\"status\":\"ok\""));
}

#[test]
fn test_error_response_serialization() {
    let error = JsonRpcError::method_not_found(RequestId::Number(1), "unknown_method");

    let json = serde_json::to_string(&error).expect("Serialization should succeed");

    assert!(json.contains("\"jsonrpc\":\"2.0\""));
    assert!(json.contains("\"error\""));
    assert!(json.contains("-32601")); // Method not found code
}

#[test]
fn test_response_no_credentials_leak() {
    let response = JsonRpcResponse::success(
        RequestId::Number(1),
        serde_json::json!({
            "content": [
                {
                    "type": "text",
                    "text": "Output from git command"
                }
            ]
        }),
    );

    let json = serde_json::to_string(&response).expect("Serialization should succeed");

    // Should not contain credential patterns
    assert!(!json.contains("ghp_"));
    assert!(!json.contains("glpat-"));
    assert!(!json.contains("password"));
    assert!(!json.contains("secret"));
}

// =============================================================================
// Server State Tests
// =============================================================================

#[test]
fn test_server_initial_state() {
    let server = McpServer::new();
    assert_eq!(server.state(), ServerState::AwaitingInit);
}

#[test]
fn test_server_default_creates_same_as_new() {
    let server1 = McpServer::new();
    let server2 = McpServer::default();

    assert_eq!(server1.state(), server2.state());
    assert_eq!(server1.state(), ServerState::AwaitingInit);
}

// =============================================================================
// Request ID Tests
// =============================================================================

#[test]
fn test_request_id_number() {
    let id = RequestId::Number(42);
    assert_eq!(format!("{id}"), "42");

    let json = serde_json::to_string(&id).expect("Serialization should succeed");
    assert_eq!(json, "42");
}

#[test]
fn test_request_id_string() {
    let id = RequestId::String("test-id".to_string());
    assert_eq!(format!("{id}"), "test-id");

    let json = serde_json::to_string(&id).expect("Serialization should succeed");
    assert_eq!(json, "\"test-id\"");
}

#[test]
fn test_request_id_equality() {
    assert_eq!(RequestId::Number(1), RequestId::Number(1));
    assert_ne!(RequestId::Number(1), RequestId::Number(2));
    assert_eq!(
        RequestId::String("a".to_string()),
        RequestId::String("a".to_string())
    );
    assert_ne!(RequestId::Number(1), RequestId::String("1".to_string()));
}

// =============================================================================
// Error Code Tests
// =============================================================================

#[test]
fn test_error_codes() {
    // Parse error (no arguments)
    let err = JsonRpcError::parse_error();
    assert_eq!(err.error.code, -32700);

    // Invalid request (optional id)
    let err = JsonRpcError::invalid_request(Some(RequestId::Number(1)));
    assert_eq!(err.error.code, -32600);

    // Method not found (id and method name)
    let err = JsonRpcError::method_not_found(RequestId::Number(1), "test_method");
    assert_eq!(err.error.code, -32601);

    // Invalid params
    let err = JsonRpcError::invalid_params(RequestId::Number(1), "test");
    assert_eq!(err.error.code, -32602);

    // Internal error
    let err = JsonRpcError::internal_error(RequestId::Number(1), "test");
    assert_eq!(err.error.code, -32603);
}

// =============================================================================
// MCP Lifecycle Tests
// =============================================================================

#[test]
fn test_validate_initialize_request() {
    // Valid initialize request
    let json = r#"{
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "test",
                "version": "1.0"
            }
        }
    }"#;

    let result = parse_message(json);
    assert!(result.is_ok());

    if let IncomingMessage::Request(req) = result.unwrap() {
        // Validate has correct structure
        let params = req.params.expect("Should have params");
        assert!(params.get("protocolVersion").is_some());
        assert!(params.get("clientInfo").is_some());
    }
}

#[test]
fn test_ping_request() {
    let json = r#"{
        "jsonrpc": "2.0",
        "id": 99,
        "method": "ping"
    }"#;

    let result = parse_message(json);
    assert!(result.is_ok());

    if let IncomingMessage::Request(req) = result.unwrap() {
        assert_eq!(req.method, "ping");
        assert_eq!(req.id, RequestId::Number(99));
    }
}

// =============================================================================
// Tool Call Validation Tests
// =============================================================================

#[test]
fn test_tool_call_with_valid_command() {
    let json = r#"{
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "git",
            "arguments": {
                "command": "status",
                "args": [],
                "working_dir": null
            }
        }
    }"#;

    let result = parse_message(json);
    assert!(result.is_ok());

    if let IncomingMessage::Request(req) = result.unwrap() {
        let params = req.params.expect("Should have params");
        let name = params.get("name").and_then(|v| v.as_str());
        assert_eq!(name, Some("git"));

        let args = params.get("arguments").expect("Should have arguments");
        assert_eq!(args.get("command").and_then(|v| v.as_str()), Some("status"));
    }
}

#[test]
fn test_tool_call_with_args() {
    let json = r#"{
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "git",
            "arguments": {
                "command": "log",
                "args": ["--oneline", "-n", "10"]
            }
        }
    }"#;

    let result = parse_message(json);
    assert!(result.is_ok());

    if let IncomingMessage::Request(req) = result.unwrap() {
        let params = req.params.expect("Should have params");
        let args = params.get("arguments").expect("Should have arguments");
        let cmd_args = args.get("args").and_then(|v| v.as_array());

        assert!(cmd_args.is_some());
        let cmd_args = cmd_args.unwrap();
        assert_eq!(cmd_args.len(), 3);
        assert_eq!(cmd_args[0].as_str(), Some("--oneline"));
    }
}
