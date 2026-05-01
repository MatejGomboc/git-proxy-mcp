# Security Model: git-proxy-mcp

> **Scope of this document:** the *technical* security design — threat model, architecture-level controls,
> and how each principle is enforced in code. For vulnerability reporting, supported versions, and what counts
> as a security issue, see the root [`SECURITY.md`](../SECURITY.md).

This document details the security design of git-proxy-mcp.

## Core Security Principles

### 1. Credentials Never Leave User's Machine

The MCP server runs on the user's machine and delegates all authentication to the operating system's credential infrastructure.

**SSH Authentication:**

```text
git2 needs to authenticate
          │
          ▼
┌─────────────────────────────┐
│ Cred::ssh_key_from_agent()  │
│                             │
│ Talks to ssh-agent process  │
│ Private key never leaves    │
│ the agent                   │
└─────────────────────────────┘
          │
          ▼
Agent signs the challenge
and returns signature
          │
          ▼
git2 uses signature for auth
```

**HTTPS Authentication:**

```text
git2 needs to authenticate
          │
          ▼
┌─────────────────────────────┐
│ Cred::credential_helper()   │
│                             │
│ Invokes system helper:      │
│ - osxkeychain (macOS)      │
│ - manager (Windows)        │
│ - libsecret (Linux)        │
└─────────────────────────────┘
          │
          ▼
Helper returns username+token
from OS credential store
          │
          ▼
git2 uses for HTTPS Basic auth
```

**What this means:**

- Our code NEVER sees SSH private keys
- Our code NEVER stores tokens
- Tokens pass through git2 but are not persisted
- Even if MCP server is compromised, no credentials to steal

### 2. Files Never Persist on MCP Server

Repository files are never written to disk. We use bare repositories and stream directly from git objects:

```rust
fn clone_and_stream(url: &str) -> Result<Archive> {
    // TempDir auto-deletes when dropped
    let temp_dir = TempDir::new()?;

    // Init BARE repo (no working tree)
    let repo = Repository::init_bare(temp_dir.path())?;

    // Fetch objects from remote (pack files only)
    fetch_remote(&repo, url)?;

    // Stream tree directly to tar (from object DB, not disk)
    let tree = repo.find_commit(head)?.tree()?;
    let archive = create_tar_from_tree(&repo, &tree)?;

    // temp_dir drops here → pack files deleted
    // Source files were NEVER on disk
    Ok(archive)
}
```

**Security benefits:**

- Source files NEVER written to user's disk
- Only git pack files in temp (compressed, deduplicated objects)
- No risk of leftover files after disconnect
- Temp directory permissions are 0700 (owner only)

### 3. Audit Trail Without Secrets

All operations are logged, but credentials are never included:

```rust
// ✅ Good: Log operation details. Audit events are constructed via
// `AuditEvent::repo_clone_success(url, branch, commit, file_count,
// archive_size, duration)` (and matching `_failed` / `_blocked`
// constructors) — see `src/security/audit.rs` for the full set.
// URLs must be sanitised by the *caller* before being passed in.
self.audit_logger.log_silent(&AuditEvent::repo_clone_success(
    sanitize_url_for_logging(url),
    branch,
    commit,
    file_count,
    archive_size,
    elapsed,
));

// ❌ Never: Log credential info
tracing::debug!("Using credential: {:?}", cred);  // NEVER
```

## Threat Model

### Threats We Protect Against

| Threat | Mitigation |
|--------|------------|
| **Credential theft** | Credentials handled by OS, never stored by us |
| **Credential logging** | Code audit; no Cred in logs; URL sanitisation |
| **Code exfiltration** | Temp files only; deleted after streaming |
| **Unauthorised repo access** | Uses user's own credentials; can only access what they can |
| **Malicious bundles** | Bundle validation before processing |
| **Path traversal** | Archive extraction validates paths |
| **Denial of service** | Rate limiting; configurable size limits |
| **Protected branch bypass** | Branch guards check before push |

### Threats We Do NOT Protect Against

| Threat | Why |
|--------|-----|
| **Malicious MCP client** | If client is compromised, game over anyway |
| **Compromised user machine** | Credentials are on the machine |
| **User misconfiguration** | If user allows force push, we allow it |
| **GitHub/GitLab compromise** | Out of scope; provider security |

## Security Controls

### 1. Branch Protection

```rust
// `BranchGuard::new` accepts any iterator of `Into<String>` patterns.
let branch_guard = BranchGuard::new(["main", "master", "release/*"]);

// Before push: the actual check returns a `SecurityCheckResult` rather
// than a bool — `Allowed` or `Blocked { reason }` — and `Blocked` is
// surfaced to the client as a tool error.
match branch_guard.check("push", &push_args) {
    SecurityCheckResult::Blocked { reason } => return Err(reason),
    SecurityCheckResult::Allowed => {}
}
```

### 2. Force Push Protection

```rust
// `PushGuard::new(false)` blocks all `--force` / `-f` /
// `--force-with-lease` pushes; `PushGuard::new(true)` allows them.
let push_guard = PushGuard::new(false);

match push_guard.check("push", &push_args) {
    SecurityCheckResult::Blocked { reason } => return Err(reason),
    SecurityCheckResult::Allowed => {}
}
```

### 3. Repository Filtering

```rust
// Allowlist mode: only allow patterns added via `allow(...)`.
let mut filter = RepoFilter::allowlist_mode();
filter.allow("github.com/myorg/*");

// Or blocklist mode (the default): block patterns added via `block(...)`,
// allow everything else.
let mut filter = RepoFilter::blocklist_mode();
filter.block("github.com/secrets/*");

if !filter.is_allowed(repo_url) {
    return Err("Repository is not allowed by policy");
}
```

### 4. Rate Limiting

```rust
// `RateLimiter::new(max_burst, refill_rate)` — no named arguments in
// Rust, just positional ones.
let limiter = RateLimiter::new(20, 5.0);

if !limiter.try_acquire() {
    return Err("Rate limit exceeded");
}
```

### 5. URL Sanitisation

The actual implementation lives at `src/git2_ops/auth.rs::sanitize_url_for_logging`
and uses byte-safe `find` operations rather than a regex (the project has no
regex dependency). The behaviour is: if the URL contains both `://` and `@`,
everything between the scheme and the `@` is replaced with `***`.

```rust
/// Remove credentials from URLs before logging.
pub fn sanitize_url_for_logging(url: &str) -> String {
    if let Some(at_pos) = url.find('@') {
        if let Some(scheme_end) = url.find("://") {
            let scheme = &url[..scheme_end + 3];
            let after_at = &url[at_pos + 1..];
            return format!("{scheme}***@{after_at}");
        }
    }
    url.to_string()
}
```

## Secure Coding Guidelines

### DO

```rust
// ✅ Use system credential helpers
Cred::credential_helper(&config, url, username)
Cred::ssh_key_from_agent(username)

// ✅ Use TempDir for automatic cleanup
let temp = TempDir::new()?;

// ✅ Sanitise URLs in logs (this project uses `tracing`, not `log`)
tracing::info!("Cloning {}", sanitize_url_for_logging(url));

// ✅ Validate paths in archives
if path.starts_with("..") {
    return Err("Invalid path");
}

// ✅ Generic error messages
Err("Authentication failed. Check your credential configuration.")
```

### DON'T

```rust
// ❌ Store credentials
self.token = get_token();  // NEVER

// ❌ Log credentials
tracing::debug!("Token: {}", token);  // NEVER
tracing::debug!("Cred: {:?}", cred);  // NEVER

// ❌ Include credentials in errors
Err(format!("Auth failed with token: {}", token))  // NEVER

// ❌ Manual temp file paths
let path = "/tmp/repo";  // Use TempDir instead
```

## Incident Response

### If Credentials Are Suspected Leaked

1. **Revoke immediately** — Rotate the affected credential
2. **Check audit logs** — Look for unauthorised operations
3. **Review code** — Find the leak source
4. **Report** — If vulnerability, follow SECURITY.md disclosure process

### If Unauthorised Push Detected

1. **Git revert** — Undo the push on the remote
2. **Check audit logs** — When and from where
3. **Review session** — Was client compromised?
4. **Strengthen guards** — Add the branch to protected list

## Security Checklist for Contributors

Before submitting code:

- [ ] No `Cred` objects logged or serialised
- [ ] No credentials in error messages
- [ ] URLs sanitised before logging
- [ ] Temp files use `TempDir` (auto-delete)
- [ ] Archive paths validated (no `..`)
- [ ] Rate limiting not bypassed
- [ ] Branch guards not bypassed
- [ ] New config options have secure defaults

## Reporting Security Issues

See [SECURITY.md](../SECURITY.md) for vulnerability disclosure process.
