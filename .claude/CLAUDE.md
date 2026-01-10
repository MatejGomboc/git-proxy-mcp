# git-proxy-mcp — AI Assistant Context

## What Is This Project?

A secure credential proxy that enables **cloud-based AI assistants** (Claude.ai, ChatGPT, Gemini) to work with private Git repositories.

**The key insight:** Cloud AIs have their own VMs with full compute capability. They can run git, build code, run tests. They just can't authenticate to private repos because they (rightfully) don't have access to user credentials.

**We solve exactly that problem:** Stream Git data through an authenticated proxy on the user's machine.

## Architecture (v2)

```
GitHub/GitLab              User's PC                  AI's VM
     │                         │                         │
     │                    ┌────┴────┐                    │
     │                    │ git-proxy│                    │
     │◄───── git2 auth ───┤   mcp   ├─── MCP stream ────►│
     │                    │         │                    │
     │                    └────┬────┘                    │
     │                         │                         │
     │                   Credentials               Full repo
     │                   stay here               lives here
```

**Three core operations:**

| Tool | What It Does |
|------|-------------|
| `repo/clone` | Stream repo from GitHub → AI's VM as tar.gz |
| `repo/push` | Stream commits from AI's VM → GitHub via bundle |
| `repo/pull` | Stream only changed files (incremental sync) |

## Who This Is For

| Environment | Uses This? | Why |
|-------------|-----------|-----|
| **Claude.ai** | ✅ YES | Has VM, lacks credentials |
| **ChatGPT** | ✅ YES | Has Code Interpreter, lacks credentials |
| **Gemini** | ✅ YES | Has code execution, lacks credentials |
| Claude Code | ❌ No | Already runs on user's machine |
| Cursor | ❌ No | Already has local access |

## Quick Reference

| Resource | Location |
|----------|----------|
| Battle plan | `TODO.md` |
| Architecture details | `docs/ARCHITECTURE.md` |
| Style guide | `STYLE.md` |
| Build commands | `CONTRIBUTING.md` |
| Commit conventions | `CONTRIBUTING.md` |

## Critical Rules

### 🔴 NEVER Do These

1. **NEVER store credentials** — Not in memory, not in config, not in session state
2. **NEVER log credentials** — No Cred objects, no tokens, no keys
3. **NEVER push to main** — Always feature branch + PR
4. **NEVER include credentials in error messages**

### 🟢 ALWAYS Do These

1. **Use git2 credential callbacks** — Let the system handle auth
2. **Clean up temp files** — Use `TempDir` which auto-deletes
3. **Follow TODO.md phases** — One feature at a time
4. **Update CHANGELOG.md** — For user-facing changes

## Project Structure

```
src/
├── git2_ops/           # NEW: git2 library integration
│   ├── mod.rs          # Module exports  
│   ├── auth.rs         # Credential callbacks (CRITICAL)
│   ├── clone.rs        # Clone and streaming
│   ├── push.rs         # Bundle handling and push
│   └── error.rs        # git2-specific errors
│
├── streaming/          # NEW: Transfer format handling
│   ├── mod.rs
│   ├── tar.rs          # Tar archive creation
│   └── bundle.rs       # Git bundle handling
│
├── mcp/                # MCP protocol (extend, don't rewrite)
│   ├── server.rs       # Add new tool handlers here
│   ├── protocol.rs     # Keep as-is
│   └── transport.rs    # Keep as-is
│
├── security/           # Keep from v1
│   ├── audit.rs        # Audit logging
│   ├── guards.rs       # Branch/push protection
│   └── rate_limit.rs   # Rate limiting
│
├── config/             # Extend for new options
│
├── session.rs          # NEW: Repo session management
│
└── main.rs             # Entry point
```

## Key Implementation Patterns

### Credential Handling (MEMORISE THIS)

```rust
// ✅ CORRECT: Use system credential helpers
let mut callbacks = git2::RemoteCallbacks::new();
callbacks.credentials(|url, username, allowed| {
    if allowed.contains(CredentialType::SSH_KEY) {
        if let Some(user) = username {
            return Cred::ssh_key_from_agent(user);
        }
    }
    if allowed.contains(CredentialType::USER_PASS_PLAINTEXT) {
        let config = git2::Config::open_default()?;
        return Cred::credential_helper(&config, url, username);
    }
    Err(git2::Error::from_str("no credential method"))
});

// ❌ WRONG: Never do these
let token = "ghp_xxxx";  // NEVER hardcode
log::debug!("Cred: {:?}", cred);  // NEVER log
session.credential = cred;  // NEVER store
```

### Streaming Clone Pattern

```rust
// Clone to temp, stream tar, delete temp
pub async fn handle_clone(url: &str) -> Result<Vec<u8>> {
    // 1. Clone to temp directory
    let temp_dir = TempDir::new()?;  // Auto-deletes on drop
    let repo = clone_with_auth(url, temp_dir.path())?;
    
    // 2. Create tar.gz archive
    let archive = create_tar_gz(temp_dir.path())?;
    
    // 3. temp_dir drops here, files deleted
    Ok(archive)
}
```

### Error Messages (Security Critical)

```rust
// ✅ CORRECT: Generic error, no secrets
Err(ToolError::AuthFailed(
    "Authentication failed. Check credential helper config.".into()
))

// ❌ WRONG: Leaks credential info
Err(ToolError::AuthFailed(
    format!("Auth failed for token: {}", token)  // NEVER
))
```

## Current Development Phase

**Phase 1: Foundation Rewrite** ← WE ARE HERE

See `TODO.md` for detailed tasks. Summary:

1. ✅ Add git2 dependency and module structure
2. 🔄 Implement credential callbacks
3. 🔄 Implement streaming clone
4. ⬜ Implement push
5. ⬜ Session management
6. ⬜ Integration tests

## Testing

```bash
# Unit tests (no network)
cargo test

# Integration tests (need credentials)
GIT_TEST_REPO_URL=https://github.com/you/test-repo cargo test --features integration

# All quality checks
cargo fmt --check
cargo clippy -- -D warnings
cargo test
```

## Useful git2 Examples

The git2 crate has excellent examples:
- https://github.com/rust-lang/git2-rs/tree/master/examples

Key ones to study:
- `clone.rs` — Basic cloning
- `fetch.rs` — Fetching with progress
- `push.rs` — Pushing with credentials

## Off Limits

**`CODE_OF_CONDUCT.md`** — Do not modify. Owned by repository owner.

## Before Committing

```bash
# Clean up merged branches
git fetch --prune origin
git branch -vv | grep ': gone]' | awk '{print $1}' | xargs -r git branch -d

# Run all checks
cargo fmt
cargo clippy -- -D warnings
cargo test

# Update changelog for user-facing changes
```

## Common Pitfalls

| Pitfall | Solution |
|---------|----------|
| git2 credential callback called multiple times | That's normal — git tries different auth methods |
| SSH auth fails | Ensure ssh-agent is running with key added |
| HTTPS auth fails | Ensure credential helper is configured |
| Large repo OOM | Use shallow clone + sparse checkout |
| Temp files not cleaned | Use `TempDir`, not manual temp paths |
| Tests fail in CI | Integration tests need `GIT_TEST_REPO_URL` env |
