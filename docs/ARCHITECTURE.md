# Architecture: git-proxy-mcp v2

This document describes the v2 architecture — a complete redesign focused on streaming Git data between providers and AI workspaces.

## Problem Statement

Cloud-based AI assistants (Claude.ai, ChatGPT, Gemini) have:

- ✅ Compute capability (Linux VMs, sandboxes)
- ✅ Ability to run git locally
- ❌ No access to user's credentials for private repos

Existing solutions are inadequate:

| Solution | Problem |
|----------|--------|
| GitHub MCP Server | File-by-file API calls. 100 files = 100 calls. Can't run tests. |
| Share credentials with AI | Security nightmare. Credentials in someone else's cloud. |
| Public repos only | Most real work is private. |

## Solution: Credential Proxy

The MCP server acts as an authenticated proxy that streams Git data:

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                              DATA FLOW                                      │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                             │
│  CLONE:                                                                     │
│  GitHub ──► git2 fetch ──► create tar.gz ──► MCP response ──► AI's VM      │
│                  ▲                                                          │
│             credentials                                                     │
│            (from system)                                                    │
│                                                                             │
│  PUSH:                                                                      │
│  AI's VM ──► git bundle ──► MCP request ──► git2 push ──► GitHub           │
│                                                  ▲                          │
│                                             credentials                     │
│                                            (from system)                    │
│                                                                             │
└─────────────────────────────────────────────────────────────────────────────┘
```

## Key Design Principles

### 1. Credentials Never Leave User's Machine

The MCP server uses git2's credential callbacks to delegate authentication to the system:

```rust
callbacks.credentials(|url, username, allowed| {
    // SSH: Use ssh-agent (key never leaves agent)
    if allowed.contains(CredentialType::SSH_KEY) {
        return Cred::ssh_key_from_agent(username.unwrap_or("git"));
    }
    
    // HTTPS: Use system credential helper
    if allowed.contains(CredentialType::USER_PASS_PLAINTEXT) {
        let config = git2::Config::open_default()?;
        return Cred::credential_helper(&config, url, username);
    }
    
    Err(git2::Error::from_str("no credential method"))
});
```

**What this means:**

- SSH private keys stay in ssh-agent (never read by our code)
- HTTPS tokens handled by OS credential store
- No credentials stored in MCP server memory
- No credentials in logs, errors, or MCP responses

### 2. Files Never Persist on User's Machine

The MCP server is a pure streaming proxy:

```rust
async fn handle_clone(url: &str) -> Result<TarArchive> {
    // 1. Clone to TEMP directory
    let temp_dir = TempDir::new()?;  // Auto-deleted on drop
    let repo = clone_repo(url, temp_dir.path())?;
    
    // 2. Create tar archive in memory
    let archive = create_tar_gz(temp_dir.path())?;
    
    // 3. Return archive (temp_dir deleted here)
    Ok(archive)
    
    // temp_dir.drop() called automatically — files gone
}
```

**Data lifecycle:**

| Phase | Data Location | Duration |
|-------|---------------|----------|
| Clone fetch | Memory (git2 objects) | Seconds |
| Working tree | Temp directory | Seconds |
| Tar archive | Memory | Seconds |
| Final storage | AI's VM only | Persistent |

### 3. AI Gets Full Git Workflow

After receiving the streamed archive, the AI has:

```
/home/claude/repo/
├── src/
│   ├── main.rs
│   └── lib.rs
├── tests/
├── Cargo.toml
├── Cargo.lock
└── README.md
```

The AI then initialises git and works locally:

```bash
# AI's workflow (all local, no network)
cd /home/claude/repo
git init
git add .
git commit -m "Initial state from clone"

# Work on the code
git checkout -b feature/fix-bug
vim src/main.rs
cargo test  # ← Can actually run tests!
git add .
git commit -m "Fix the bug"

# Ready to push - create bundle
git bundle create changes.bundle origin/main..HEAD
# Send bundle to MCP server for authenticated push
```

## Component Architecture

### MCP Tools

```
┌─────────────────────────────────────────────────────────────────┐
│ MCP Tool: repo/clone                                            │
├─────────────────────────────────────────────────────────────────┤
│ Input:                                                          │
│   url: string        "https://github.com/user/repo"            │
│   branch?: string    "main"                                    │
│   depth?: number     1 (shallow)                               │
│   sparse?: string[]  ["src/", "Cargo.toml"]                    │
│                                                                 │
│ Output:                                                         │
│   archive: string    Base64-encoded tar.gz                     │
│   commit: string     "abc123..."                               │
│   branch: string     "main"                                    │
│   file_count: number 47                                        │
└─────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────┐
│ MCP Tool: repo/push                                             │
├─────────────────────────────────────────────────────────────────┤
│ Input:                                                          │
│   url: string        "https://github.com/user/repo"            │
│   branch: string     "feature/fix-bug"                         │
│   bundle: string     Base64-encoded git bundle                 │
│                                                                 │
│ Output:                                                         │
│   success: boolean   true                                      │
│   commit: string     "def456..."                               │
│   url: string        "https://github.com/.../commit/def456"   │
└─────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────┐
│ MCP Tool: repo/pull                                             │
├─────────────────────────────────────────────────────────────────┤
│ Input:                                                          │
│   url: string        "https://github.com/user/repo"            │
│   branch: string     "main"                                    │
│   since_commit: str  "abc123..." (what AI has)                 │
│                                                                 │
│ Output:                                                         │
│   archive: string    Base64 tar.gz of changed files only       │
│   commit: string     New HEAD commit                           │
│   changed: string[]  List of changed file paths                │
└─────────────────────────────────────────────────────────────────┘
```

### Internal Modules

```
src/
├── git2_ops/                    # git2 library wrapper
│   ├── mod.rs                   # Public API
│   ├── auth.rs                  # Credential callbacks
│   │   └── create_callbacks()   # Returns configured RemoteCallbacks
│   ├── clone.rs                 # Clone operations
│   │   ├── clone_repo()         # Clone to temp dir
│   │   └── CloneOptions         # URL, branch, depth, sparse
│   ├── push.rs                  # Push operations
│   │   └── push_bundle()        # Apply bundle and push
│   └── error.rs                 # git2-specific errors
│
├── streaming/                   # Transfer format handling
│   ├── mod.rs
│   ├── tar.rs                   # Tar archive creation
│   │   ├── create_tar_gz()      # Dir → tar.gz bytes
│   │   └── TarOptions           # Exclude patterns, etc.
│   └── bundle.rs                # Git bundle handling
│       ├── create_bundle()      # Repo → bundle bytes
│       └── apply_bundle()       # Bundle → repo
│
├── mcp/                         # MCP protocol layer
│   ├── server.rs                # Main server, tool dispatch
│   ├── tools/                   # Tool handlers
│   │   ├── repo_clone.rs        # repo/clone handler
│   │   ├── repo_push.rs         # repo/push handler
│   │   └── repo_pull.rs         # repo/pull handler
│   ├── protocol.rs              # JSON-RPC types (unchanged)
│   └── transport.rs             # stdio transport (unchanged)
│
├── security/                    # Security guards (from v1)
│   ├── audit.rs                 # Audit logging
│   ├── guards.rs                # Branch/push protection
│   └── rate_limit.rs            # Rate limiting
│
├── session.rs                   # Repo session tracking
│   ├── SessionManager           # Track active repos
│   └── RepoSession              # URL, branch, last_commit
│
└── config/                      # Configuration (extend v1)
    └── settings.rs              # Add new options
```

## Session Management

**Why sessions?** To enable incremental operations:

```rust
pub struct RepoSession {
    pub url: String,
    pub branch: String,
    pub last_commit: String,  // What the AI has
    pub cloned_at: Instant,
}

pub struct SessionManager {
    sessions: HashMap<String, RepoSession>,  // key = "url:branch"
}
```

**Workflow:**

1. AI calls `repo/clone` → Session created with `last_commit`
2. AI works locally, creates commits
3. AI calls `repo/push` → Session updated with new `last_commit`
4. Later, AI calls `repo/pull` → We fetch only commits after `last_commit`

**What sessions DON'T store:**

- ❌ Credentials (never)
- ❌ File contents (AI has those)
- ❌ Repository objects (AI has those)

## Security Model

### Threat Model

| Threat | Mitigation |
|--------|------------|
| Credential theft | Credentials never stored; system helpers only |
| Credential logging | Audit code; Cred objects never in logs |
| Man-in-the-middle | TLS to Git providers; stdio to MCP client |
| Malicious bundle | Validate bundle format before processing |
| Path traversal | Validate archive paths; no `..` allowed |
| DoS via large repo | Rate limiting; configurable size limits |
| Unauthorized branch push | Branch protection guards |

### Credential Flow (Detailed)

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ SSH Authentication                                                          │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                             │
│  git2 needs auth ──► callback invoked ──► Cred::ssh_key_from_agent()       │
│                                                    │                        │
│                                                    ▼                        │
│                                           ssh-agent (OS process)            │
│                                                    │                        │
│                                           Signs challenge                   │
│                                           (key never leaves agent)          │
│                                                    │                        │
│                                                    ▼                        │
│                                           Signed response ──► git2 ──► GitHub
│                                                                             │
│  Private key NEVER read by git-proxy-mcp                                   │
│                                                                             │
└─────────────────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────────────────┐
│ HTTPS Authentication                                                        │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                             │
│  git2 needs auth ──► callback invoked ──► Cred::credential_helper()        │
│                                                    │                        │
│                                                    ▼                        │
│                                           OS credential helper              │
│                                           (osxkeychain / manager / etc.)    │
│                                                    │                        │
│                                           Returns username + token          │
│                                                    │                        │
│                                                    ▼                        │
│                                           git2 uses for HTTPS auth          │
│                                                                             │
│  Token passes through git2 but is NEVER stored/logged by git-proxy-mcp    │
│                                                                             │
└─────────────────────────────────────────────────────────────────────────────┘
```

## Performance Considerations

### Memory Usage

| Component | Memory Usage | Notes |
|-----------|-------------|-------|
| git2 fetch | O(repo size) | Streamed, but needs buffer |
| Tar creation | O(working tree) | Files read into memory |
| Base64 encoding | 1.33x archive size | Standard overhead |

**For large repos:** Use shallow clone + sparse checkout to limit data.

### Transfer Size Comparison

| Repo Size | GitHub MCP | git-proxy-mcp |
|-----------|------------|---------------|
| 100 files, 1MB | ~100 API calls | 1 call, ~1MB |
| 1000 files, 50MB | ~1000 API calls | 1 call, ~50MB |
| 10000 files, 500MB | Infeasible | Chunked transfer |

### Chunked Transfer (for large repos)

```rust
// For repos > MAX_SINGLE_RESPONSE (e.g., 50MB)
pub struct ChunkedCloneResult {
    pub chunk_index: usize,
    pub total_chunks: usize,
    pub data: String,  // Base64 chunk
    pub is_last: bool,
}
```

AI accumulates chunks and reassembles the archive.

## Comparison with v1

| Aspect | v1 (CLI Proxy) | v2 (Streaming Proxy) |
|--------|----------------|----------------------|
| Git library | Subprocess (`git` CLI) | git2 (in-process) |
| Use case | Local MCP clients | Cloud AI assistants |
| Files on MCP machine | Cloned to disk | Never persisted |
| Transfer method | CLI stdout | Tar/bundle streaming |
| Credential handling | Env vars | git2 callbacks |
| Output sanitisation | Regex on stdout | Not needed (binary) |

## Future Considerations

### WebSocket Transport

Currently stdio only. Future: WebSocket for remote MCP clients.

### Provider-Specific Features

- Create pull request (requires GitHub/GitLab API)
- Fork repository
- Manage webhooks

These may need separate provider API integration beyond git2.

### Git LFS

Large File Storage needs special handling:

1. Detect LFS pointers in tree
2. Fetch actual objects from LFS server
3. Stream with regular files

Deferred to Phase 5.

## References

- [git2-rs documentation](https://docs.rs/git2/latest/git2/)
- [git2-rs examples](https://github.com/rust-lang/git2-rs/tree/master/examples)
- [libgit2 authentication guide](https://libgit2.org/docs/guides/authentication/)
- [Git bundle format](https://git-scm.com/docs/git-bundle)
- [Git transfer protocols](https://git-scm.com/book/en/v2/Git-Internals-Transfer-Protocols)
- [MCP specification](https://modelcontextprotocol.io/)
