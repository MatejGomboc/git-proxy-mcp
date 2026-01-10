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
| GitHub MCP Server | File-by-file API calls. Slow. Can't run tests. |
| Share credentials with AI | Security nightmare. |
| Public repos only | Most real work is private. |

## Solution: Credential Proxy

The MCP server acts as an authenticated proxy that streams Git data:

```
┌─────────────────────────────────────────────────────────────────────────┐
│                           DATA FLOW                                     │
├─────────────────────────────────────────────────────────────────────────┤
│                                                                         │
│   CLONE:  GitHub ──► git2 fetch ──► stream tar.gz ──► AI's VM          │
│                          ▲                                              │
│                     credentials                                         │
│                     (never leave)                                       │
│                                                                         │
│   PUSH:   AI's VM ──► patches ──► git2 push ──► GitHub                 │
│                                       ▲                                 │
│                                  credentials                            │
│                                  (never leave)                          │
│                                                                         │
└─────────────────────────────────────────────────────────────────────────┘
```

## Key Design Principles

### 1. Credentials Never Leave

The MCP server runs on the user's machine and uses their existing Git credential configuration:

```rust
// git2 credential callback — delegates to system helpers
callbacks.credentials(|url, username, allowed| {
    git2::Cred::credential_helper(&config, url, username)
});
```

Credentials flow:
- User's credential helper → git2 → remote
- Never serialised, logged, or transmitted via MCP

### 2. Files Never Persist

The MCP server is a pure proxy:

```rust
// Stream tree contents directly — no temp files
for entry in tree.iter() {
    let blob = repo.find_blob(entry.id())?;
    stream.write_all(blob.content())?;  // Direct to MCP response
}
```

Data flow:
- Clone: Remote → memory → MCP stream → AI's VM
- Push: AI's patches → memory → remote
- No files written to user's disk

### 3. AI Gets Full Git

The AI receives a complete repository:

```
/home/claude/repo/
├── .git/           # Full git metadata
├── src/
├── Cargo.toml
└── ...
```

The AI can then:
- `git checkout -b feature`
- `git commit -m "fix"`
- `git log`, `git diff`, etc.
- Run tests, build, lint

All local operations — no network needed.

## Components

### MCP Tools

| Tool | Input | Output |
|------|-------|--------|
| `repo/clone` | URL, branch, depth, sparse paths | Streamed tar.gz |
| `repo/push` | URL, branch, patches | Commit URLs |
| `repo/pull` | URL, since_commit | Streamed delta |

### git2 Integration

```
src/git2/
├── auth.rs       # Credential callbacks
├── clone.rs      # Remote fetch + tree streaming  
├── push.rs       # Patch application + push
├── session.rs    # Active repo tracking
└── progress.rs   # Transfer progress reporting
```

### Streaming Layer

```
src/streaming/
├── tar.rs        # Tar archive creation/extraction
├── chunked.rs    # Large transfer chunking
└── delta.rs      # Incremental sync
```

## Session Management

Stateful sessions track active repositories:

```rust
struct RepoSession {
    url: String,
    branch: String,
    last_commit: Option<Oid>,
    // NO working tree, NO object store on MCP side
}
```

Benefits:
- Incremental pull knows what AI already has
- Push knows the base commit
- No re-authentication per operation

## Security Model

| Concern | Mitigation |
|---------|------------|
| Credential leakage | Never serialised; git2 callbacks only |
| Man-in-the-middle | TLS to remotes; stdio to MCP client |
| Malicious patches | Validate before apply; guard protected branches |
| Audit trail | Log all operations (no credentials) |
| Rate limiting | Prevent abuse |

## Performance Targets

| Operation | Target | vs GitHub MCP |
|-----------|--------|---------------|
| Clone 100 files | < 5s | 10-50x faster |
| Push 10 commits | < 3s | 5-10x faster |
| Incremental pull | < 1s | N/A (not supported) |

## Comparison with v1

| Aspect | v1 (CLI proxy) | v2 (Streaming proxy) |
|--------|----------------|---------------------|
| Git library | Subprocess (`git` CLI) | git2 (in-process) |
| Files on MCP | Cloned to disk | Never persisted |
| Use case | Local MCP clients | Cloud AI assistants |
| Transfer | CLI output | Streaming tar/patches |

## Future Considerations

- **WebSocket transport**: For non-stdio MCP clients
- **Provider APIs**: Create PRs, manage branches
- **LFS support**: Large file handling
- **Submodules**: Recursive clone support
