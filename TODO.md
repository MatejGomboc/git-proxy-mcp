# TODO — Development Battle Plan

## Overview

**Goal:** Build a secure credential proxy that enables cloud-based AI assistants to work with private Git repositories. The AI maintains a full local repo in its VM; the MCP server only handles authentication.

**Target Users:** Cloud AI assistants with compute capability but no credential access:
- Claude.ai (with computer use)
- ChatGPT with Code Interpreter
- Gemini with code execution
- Any sandboxed AI environment

**Non-Targets:** Local AI tools (Claude Code, Cursor, Aider) — they already have direct Git access.

**Guiding Principles:**

- Credentials NEVER leave the user's machine
- Files NEVER persist on the user's machine (stream-through only)
- AI gets a complete local git workflow in its VM
- Beat GitHub MCP Server in every metric that matters

---

## Architecture: Pure Credential Proxy

```
GIT PROVIDER                 YOUR PC                    AI's VM
(GitHub/GitLab)             (MCP Server)              (Claude.ai)

┌──────────────┐      ┌─────────────────┐      ┌─────────────────┐
│              │      │ git-proxy-mcp   │      │                 │
│ repo objects │◄────►│                 │◄────►│ /home/claude/   │
│              │      │ • Credentials   │      │   repo/         │
│              │      │ • git2 library  │      │   .git/         │
│              │      │ • NO storage    │      │   (full repo)   │
└──────────────┘      └─────────────────┘      └─────────────────┘
                             │                        │
                      Credentials stay          Files live
                      on YOUR machine           in AI's VM
```

### Data Flow

| Operation | Flow |
|-----------|------|
| Clone | GitHub → (git2 auth) → MCP streams → AI's VM |
| Push | AI's VM → patches → MCP → (git2 auth) → GitHub |
| Pull | GitHub → (git2 auth) → MCP streams delta → AI's VM |

---

## Design Decisions (v2)

| Decision | Choice | Rationale |
|----------|--------|----------|
| Git library | `git2` (libgit2) | In-process, streaming, no subprocess overhead |
| File storage on MCP | None | Pure proxy — stream through only |
| Transfer format (clone) | tar.gz stream | Simple, fast, one blob |
| Transfer format (push) | git format-patch | Preserves commit metadata |
| Session state | Stateful | Track active repos, avoid re-auth |
| Credential storage | System helpers | Use existing git credential config |
| Large repo handling | Shallow + sparse | User-configurable limits |

---

## Phase 1: Foundation Rewrite ← CURRENT

### 1.1 Add git2 Dependency

- [ ] Add `git2 = "0.19"` to Cargo.toml
- [ ] Create `src/git2/` module structure
- [ ] Implement credential callback using system helpers
- [ ] Test basic authentication against GitHub/GitLab

### 1.2 Implement Streaming Clone

- [ ] Create `repo/clone` MCP tool
- [ ] Use git2 to connect to remote and fetch objects
- [ ] Stream tree contents directly (no disk write on MCP side)
- [ ] Package as tar.gz for transport
- [ ] Handle progress reporting

### 1.3 Implement Push

- [ ] Create `repo/push` MCP tool
- [ ] Receive patches/diff from AI
- [ ] Use git2 to create commits in memory
- [ ] Push to remote with authentication
- [ ] Return commit URLs

### 1.4 Basic Session Management

- [ ] Track active repo sessions (URL, branch, last commit)
- [ ] Session cleanup on disconnect
- [ ] Concurrent session support

---

## Phase 2: Full Workflow Support

### 2.1 Incremental Sync (Pull)

- [ ] Create `repo/pull` MCP tool
- [ ] Fetch new commits since last known
- [ ] Stream only changed files (delta)
- [ ] Handle merge conflicts gracefully

### 2.2 Shallow Clone Support

- [ ] Add `depth` parameter to clone
- [ ] Implement shallow fetch with git2
- [ ] Document limitations of shallow repos

### 2.3 Sparse Checkout Support

- [ ] Add `sparse` parameter (path patterns)
- [ ] Filter tree walk to only requested paths
- [ ] Significant performance gain for large repos

---

## Phase 3: Production Hardening

### 3.1 Chunked Transfer

- [ ] Handle repos larger than MCP message limits
- [ ] Implement chunked streaming
- [ ] Resume support for interrupted transfers

### 3.2 Error Handling

- [ ] Graceful handling of network failures
- [ ] Auth failure messages (don't leak credential details)
- [ ] Timeout handling for large operations

### 3.3 Security Audit

- [ ] Ensure no credential leakage in any code path
- [ ] Audit git2 callback handling
- [ ] Review streaming for memory safety
- [ ] Rate limiting for abuse prevention

### 3.4 Logging & Audit

- [ ] Audit log all operations (repo, branch, success/fail)
- [ ] Structured logging for debugging
- [ ] No credentials in any log output

---

## Phase 4: Provider Support

### 4.1 GitHub

- [ ] HTTPS with PAT authentication
- [ ] SSH key authentication
- [ ] GitHub-specific error messages

### 4.2 GitLab

- [ ] HTTPS with PAT authentication
- [ ] SSH key authentication
- [ ] Self-hosted GitLab support

### 4.3 Other Providers

- [ ] Bitbucket (if demand)
- [ ] Azure DevOps (if demand)
- [ ] Generic Git server support

---

## Phase 5: Advanced Features

### 5.1 Branch Operations

- [ ] Create branch on remote
- [ ] Delete branch on remote
- [ ] List remote branches

### 5.2 PR/MR Integration

- [ ] Create pull request (via provider API)
- [ ] This may need provider-specific MCP tools

### 5.3 Submodule Support

- [ ] Detect submodules
- [ ] Optional recursive clone
- [ ] Document limitations

### 5.4 LFS Support

- [ ] Detect LFS files
- [ ] Stream LFS objects
- [ ] May require separate handling

---

## Migration from v1

The v1 implementation (git CLI proxy) will be deprecated:

| v1 Component | v2 Status |
|--------------|----------|
| `src/git/command.rs` | Remove — no longer spawning CLI |
| `src/git/executor.rs` | Remove — using git2 instead |
| `src/git/sanitiser.rs` | Keep — still useful for output |
| `src/security/*` | Keep — guards still apply |
| `src/mcp/*` | Refactor — new tool definitions |
| `src/config/*` | Keep — extend for new options |

---

## Success Metrics

| Metric | Target |
|--------|--------|
| Clone 100 files | < 5 seconds |
| Push 10 commits | < 3 seconds |
| Incremental pull | < 1 second |
| Memory usage (MCP server) | < 50 MB (streaming, not buffering) |
| GitHub MCP comparison | 10x+ faster for typical workflows |

---

## Out of Scope

- Web UI features (GitHub issues, PRs via web) — use GitHub MCP for that
- Local AI tools — they don't need this
- Credential storage — always use system helpers
- File persistence on MCP server — pure proxy only

---

## References

- [git2 (libgit2 Rust bindings)](https://crates.io/crates/git2)
- [libgit2 documentation](https://libgit2.org/)
- [MCP Specification](https://modelcontextprotocol.io/)
- [Git transfer protocols](https://git-scm.com/book/en/v2/Git-Internals-Transfer-Protocols)

---

*Last updated: 2026-01-10*
