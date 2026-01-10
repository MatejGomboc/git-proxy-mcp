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
| Rate limit exceeded | `Rate limit exceeded. Please wait before sending more Git commands.` |

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

When a git command is executed, the result is returned in the tool call response. Errors are indicated by `isError: true`:

### Success Response

```json
{
    "content": [
        {
            "type": "text",
            "text": "Cloning into 'repo'...\nRemote: Counting objects: 100, done."
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
            "text": "Command failed with exit code 128:\nfatal: repository 'https://github.com/nonexistent/repo.git/' not found"
        }
    ],
    "isError": true
}
```

### Common Git Exit Codes

| Exit Code | Meaning |
|-----------|---------|
| 0 | Success |
| 1 | Generic error |
| 128 | Fatal error (e.g., repository not found, authentication failure) |

---

## MCP Lifecycle Errors

These errors occur during the MCP handshake.

| Error | Message | Cause |
|-------|---------|-------|
| Already initialised | `Server already initialised` | `initialize` called more than once |
| Not initialised | `Server not initialised` | `tools/call` before completing handshake |
| Missing params | `Missing initialize params` | `initialize` request has no params |
| Invalid params | `Invalid initialize params: {error}` | Malformed initialization parameters |

---

## Credential Sanitisation

Output is sanitised to prevent credential leakage. When credentials are detected, they are replaced with `[REDACTED]`.

### Detected Patterns

| Pattern | Description |
|---------|-------------|
| `ghp_*`, `gho_*`, `ghu_*`, `ghs_*`, `ghr_*` | GitHub tokens |
| `glpat-*`, `gloas-*`, `gldt-*`, `glrt-*`, `glcbt-*` | GitLab tokens |
| `ATBB*` | Bitbucket app passwords |
| `azure://` | Azure DevOps URLs |
| `https://user:pass@host` | URL-embedded credentials |
| `Authorization:`, `Bearer` | HTTP auth headers |
| `-----BEGIN * PRIVATE KEY` | SSH/PGP keys |

### Example

Input:

```text
error: https://user:ghp_secret123@github.com failed
```

Output:

```text
error: https://[REDACTED]@github.com failed
```

---

## Troubleshooting

### "git command not found"

Ensure Git is installed and in your PATH:

```bash
git --version
```

### Authentication Failures

git-proxy-mcp uses your existing Git configuration. Test authentication:

```bash
# For HTTPS
git ls-remote https://github.com/your-private-repo.git

# For SSH
git ls-remote git@github.com:your-private-repo.git
```

If prompted for credentials, configure a credential helper or SSH agent.

### "terminal prompts disabled"

This error means Git tried to prompt for credentials interactively. git-proxy-mcp sets `GIT_TERMINAL_PROMPT=0` to prevent hanging.

**Solution:** Configure a credential helper to cache your credentials:

```bash
# macOS
git config --global credential.helper osxkeychain

# Windows
git config --global credential.helper manager

# Linux
git config --global credential.helper libsecret
```

---

*Last updated: 2026-01-01*
