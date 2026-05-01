# Security Model: git-proxy-mcp

> **Scope of this document:** the *technical* security design — threat model, architecture-level controls,
> and how each principle is enforced in code. For vulnerability reporting, supported versions, and what counts
> as a security issue, see the root [`SECURITY.md`](../SECURITY.md).

This document details the security design of git-proxy-mcp.

## Core Security Principles

### 1. Credentials Never Leave User's Machine

The MCP server runs on the user's machine and delegates all authentication to the operating system's credential infrastructure.

**SSH Authentication:**

```
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

```
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
// ✅ Good: Log operation details
audit_log.info("repo/clone", json!({
    "url": sanitize_url(url),  // Removes credentials from URL
    "branch": branch,
    "success": true,
    "duration_ms": elapsed,
}));

// ❌ Never: Log credential info
audit_log.debug("Using credential: {:?}", cred);  // NEVER
```

## Threat Model

### Threats We Protect Against

| Threat | Mitigation |
|--------|------------|
| **Credential theft** | Credentials handled by OS, never stored by us |
| **Credential logging** | Code audit; no Cred in logs; URL sanitisation |
| **Code exfiltration** | Temp files only; deleted after streaming |
| **Unauthorized repo access** | Uses user's own credentials; can only access what they can |
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
let branch_guard = BranchGuard::new(vec![
    "main".to_string(),
    "master".to_string(),
    "release/*".to_string(),
]);

// Before push
if branch_guard.is_protected(branch) && !is_fast_forward {
    return Err("Cannot force-push to protected branch");
}
```

### 2. Force Push Protection

```rust
let push_guard = PushGuard::new(allow_force_push: false);

// Before push
if push_guard.is_force_push(args) {
    return Err("Force push is disabled");
}
```

### 3. Repository Filtering

```rust
// Allowlist mode: only specified repos
let filter = RepoFilter::allowlist(vec![
    "github.com/myorg/*",
]);

// Or blocklist mode
let filter = RepoFilter::blocklist(vec![
    "github.com/secrets/*",
]);
```

### 4. Rate Limiting

```rust
let limiter = RateLimiter::new(
    max_burst: 20,
    refill_per_sec: 5.0,
);

// Before operation
if !limiter.try_acquire() {
    return Err("Rate limit exceeded");
}
```

### 5. URL Sanitisation

```rust
/// Remove credentials from URLs before logging
pub fn sanitize_url(url: &str) -> String {
    // https://user:token@github.com/... → https://***@github.com/...
    let re = Regex::new(r"://[^@]+@").unwrap();
    re.replace(url, "://***@").to_string()
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

// ✅ Sanitise URLs in logs
log::info!("Cloning {}", sanitize_url(url));

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
log::debug!("Token: {}", token);  // NEVER
log::debug!("Cred: {:?}", cred);  // NEVER

// ❌ Include credentials in errors
Err(format!("Auth failed with token: {}", token))  // NEVER

// ❌ Manual temp file paths
let path = "/tmp/repo";  // Use TempDir instead
```

## Incident Response

### If Credentials Are Suspected Leaked

1. **Revoke immediately** — Rotate the affected credential
2. **Check audit logs** — Look for unauthorized operations
3. **Review code** — Find the leak source
4. **Report** — If vulnerability, follow SECURITY.md disclosure process

### If Unauthorized Push Detected

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
