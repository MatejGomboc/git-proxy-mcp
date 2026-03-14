# Error Reference

This document provides a comprehensive reference for all error messages and error codes in git-proxy-mcp.

---

## JSON-RPC Protocol Errors

These are standard JSON-RPC 2.0 error codes returned when there's an issue with the protocol layer.

| Code | Name | Description |
|------|------|-------------|
| -32700 | Parse error | Invalid JSON was received |
| -32600 | Invalid Request | The JSON is not a valid Request object (missing `jsonrpc: "2.0"`, missing `id`, etc.) |
| -32601 | Method not found | The requested method does not exist (e.g., `tools/unknown`) |
| -32602 | Invalid params | Invalid method parameters (e.g., missing required fields, wrong types) |
| -32603 | Internal error | Internal server error during request processing |

### Example Error Response

```json
{
    "jsonrpc": "2.0",
    "id": 1,
    "error": {
        "code": -32601,
        "message": "Method not found: unknown/method"
    }
}
```

---

## Security Guard Errors

These errors occur when security policies block an operation.

### Rate Limiting

| Error | Message |
|-------|---------|
| Rate limit exceeded | `Rate limit exceeded. Please wait before sending more requests.` |

Default rate limits: 20 operations burst, 5 operations per second sustained.

### Branch Protection

| Error | Message Format |
|-------|---------------|
| Delete protected branch | `Cannot delete protected branch '{branch}'` |
| Force push to protected branch | `Cannot force push to protected branch '{branch}'` |

Default protected branches: `main`, `master`, `develop`.

### Force Push Blocking

| Error | Message |
|-------|---------|
| Force push blocked | `Force push is not allowed. Use --force-with-lease for safer updates, or contact your administrator to enable force push.` |

Force push is blocked by default. Enable with `security.allow_force_push: true` in configuration.

### Repository Filtering

| Error | Message Format |
|-------|---------------|
| Repository blocked | `Repository '{url}' is not allowed by policy` |

Configure with `security.repo_allowlist` or `security.repo_blocklist` in configuration.

---

## Configuration Errors

These errors occur when loading or validating the configuration file.

| Error | Message Format | Cause |
|-------|---------------|-------|
| Read error | `failed to read configuration file: {path}` | Cannot read the file (permissions, IO error) |
| Parse error | `failed to parse configuration file: {path}` | Invalid JSON syntax in config file |
| Not found | `configuration file not found: {path}` | Config file doesn't exist at specified path |
| Validation error | `configuration validation failed: {message}` | Configuration values are invalid |

---

## Tool Call Results

Tool call results are returned in the tool call response. Errors are indicated by `isError: true`:

### Success Response

```json
{
    "content": [
        {
            "type": "text",
            "text": "{\"archive\": \"H4sI...\", \"commit\": \"abc123\", \"file_count\": 47}"
        }
    ]
}
```

### Error Response

```json
{
    "content": [
        {
            "type": "text",
            "text": "Repository not found or access denied"
        }
    ],
    "isError": true
}
```

---

## MCP Lifecycle Errors

These errors occur during the MCP handshake.

| Error | Message | Cause |
|-------|---------|-------|
| Already initialised | `Server already initialised` | `initialize` called more than once |
| Not initialised | `Server not initialised` | `tools/call` before completing handshake |
| Missing params | `Missing initialize params` | `initialize` request has no params |
| Invalid params | `Invalid initialize params: {error}` | Malformed initialisation parameters |

---

## URL Sanitisation

URLs are sanitised before logging to prevent credential leakage:

### Example

Input:

```text
https://user:ghp_secret123@github.com/repo.git
```

Output (in logs):

```text
https://***@github.com/repo.git
```

Credentials embedded in URLs are replaced with `***` before any logging occurs.

---

## Troubleshooting

### Authentication Failures

git-proxy-mcp uses your existing Git credential configuration via the git2 library. Test your credentials work:

```bash
# For HTTPS
git ls-remote https://github.com/your-private-repo.git

# For SSH
git ls-remote git@github.com:your-private-repo.git
```

If prompted for credentials, configure a credential helper or SSH agent.

### Credential Helper Setup

Configure a credential helper to store your credentials:

```bash
# macOS
git config --global credential.helper osxkeychain

# Windows
git config --global credential.helper manager

# Linux
git config --global credential.helper libsecret
```

### SSH Agent Setup

For SSH authentication, ensure your SSH agent is running and has your key loaded:

```bash
# Start the agent (if not running)
eval "$(ssh-agent -s)"

# Add your key
ssh-add ~/.ssh/id_ed25519
```

---

*Last updated: 2026-01-10*
