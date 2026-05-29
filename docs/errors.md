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

Effective default: `main`, `master`, `develop` are protected. The
`security.protected_branches` field in config defaults to an empty list,
but the server treats an empty list as "use the built-in safe set" and
substitutes `BranchGuard::with_defaults()` — see
`McpServer::new` in `src/mcp/server.rs`. Configuring any non-empty list
overrides the fallback. The shipped `config/example-config.json`
recommends `["main", "master"]`.

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
| Read error | `failed to read configuration file: {path}` | Cannot read the file (permissions, IO error, or the path is a directory) |
| Parse error | `failed to parse configuration file: {path}` | Invalid JSON syntax, or an unknown/mistyped field (every section uses `deny_unknown_fields`) |
| Not found | `configuration file not found: {path}` | Config file doesn't exist at specified path |
| Validation error | `configuration validation failed: {message}` | A value is out of range — see below |

### Validation rules

After parsing, the configuration is range-checked. The `{message}` names the
offending field. Only values that would render a subsystem unusable (or panic)
are rejected; values the consuming code already handles (`submodules.max_concurrent`,
`submodules.max_failures`, `lfs.retry_max_attempts`, `lfs.max_object_size`) are
accepted as-is.

| Field(s) | Rule | Why |
|----------|------|-----|
| `timeouts.request_timeout_secs`, `lfs.request_timeout_secs`, `lfs.connect_timeout_secs`, `lfs.download_timeout_secs` | must be > 0 | a zero `Duration` makes every request time out immediately |
| `limits.max_output_bytes` | must be > 0 | a zero limit would truncate every command's combined stdout+stderr to nothing |
| `rate_limits.max_burst` | must be > 0 | the token bucket would never hand out a token, blocking every operation |
| `rate_limits.refill_rate_per_sec` | finite and ≥ 0 | `NaN` panics in `time_until_available`; `±∞`/negatives break the token-bucket maths; `0.0` is allowed ("burst once, never refill") |
| `sessions.timeout_secs`, `sessions.max_streaming_sessions`, `sessions.max_repo_sessions` | must be > 0 | sessions would expire instantly or never be creatable |
| `logging.level` | one of `trace`, `debug`, `info`, `warn`, `error` (case-insensitive) | an unknown level would otherwise silently fall back to `warn`, masking a typo |

#### Example

```text
configuration validation failed: rate_limits.max_burst must be greater than 0
```

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

## LFS Resolution Errors

When `repo_clone` or `repo_clone_start` is invoked with `resolve_lfs: true`,
the server fetches LFS content from `<repo_url>.git/info/lfs/objects/batch`.
Two log lines help diagnose failures.

### Missing credentials warning

If the `LfsClient` is constructed without credentials, the server emits
a `WARN` at the start of the clone (before any batch request):

```text
WARN LFS client created without credentials — batch API requests to private
     repos will likely return 401/403
```

For private repos this is almost always the cause of any subsequent
401/403 from the batch endpoint.

### Batch-API error includes response body

Non-retryable status codes (4xx, plus 5xx after retry exhaustion) log
the HTTP status, the request URL, and the response body:

```text
WARN LFS batch POST returned non-retryable error status
     status=401 Unauthorized
     url=https://github.com/owner/repo.git/info/lfs/objects/batch
     response_body={"message":"Bad credentials"}
```

The body comes from the LFS server's response and is also included in
the returned `Git2Error`, so it surfaces in the tool-call error too. The
Authorization header is in the *request*, never the response — the
logged body cannot leak the PAT.

Common GitHub responses to recognise:

| HTTP code | Typical body | Likely cause |
|---|---|---|
| 401 | `{"message":"Bad credentials"}` | Token expired or invalid |
| 403 | `{"message":"Repository access blocked"}` | PAT lacks `repo` scope (classic) or `Contents: Read and write` (fine-grained) |
| 404 | `{"message":"Object does not exist"}` | LFS object never uploaded to the remote |
| 422 | `<!DOCTYPE html>...` | URL missing `.git` — request reached the web frontend, not the LFS service |

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

If prompted for credentials, you need to configure an OS credential helper
(HTTPS) or load your key into ssh-agent (SSH). See the README's
[Git authentication](../README.md#git-authentication) section for the
per-platform setup commands.

---

*Last updated: 2026-05-29*
