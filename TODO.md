# TODO — Development Battle Plan

## Overview

**Goal:** Build a secure, AI-agnostic Git proxy MCP server in Rust that keeps credentials on the user's machine while allowing AI assistants to work with repos in their own environments.

**Guiding Principles:**
- Security over speed. Take the time to do it right.
- Work on ONE feature at a time. See `.claude/features.json` for tracking.
- Use British spelling in documentation and user-facing text. It's posh! 🇬🇧

**For AI Assistants:** See `.claude/CLAUDE.md` for project context.

---

## Feature Tracking

Features are tracked in `.claude/features.json` with pass/fail status. 

**Rules:**
- Only change the `passes` field when a feature is verified complete
- Do NOT remove or edit feature descriptions
- Work on ONE feature at a time
- Verify each feature works before marking it as passing

---

## Security Architecture

### Credential Isolation — CRITICAL

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                              User's PC                                      │
│                                                                             │
│   ┌─────────────────────────────────────────────────────────────────────┐  │
│   │                     git-proxy-mcp                                    │  │
│   │                                                                      │  │
│   │   config.json ──┐                                                    │  │
│   │   (PAT, keys)   │  NEVER                                             │  │
│   │                 │  LEAVES ──────────────────────┐                    │  │
│   │                 ▼  HERE                         │                    │  │
│   │          ┌─────────────┐                        │                    │  │
│   │          │ Auth Module │                        │                    │  │
│   │          │ (internal)  │                        │                    │  │
│   │          └──────┬──────┘                        │                    │  │
│   │                 │                               │                    │  │
│   │                 │ HTTPS + PAT                   │                    │  │
│   │                 ▼                               │                    │  │
│   │          ┌─────────────┐                        │                    │  │
│   │          │   GitHub    │                        │                    │  │
│   │          │   GitLab    │                        │                    │  │
│   │          └──────┬──────┘                        │                    │  │
│   │                 │                               │                    │  │
│   │                 │ Git pack data                 │                    │  │
│   │                 │ (files, commits)              │                    │  │
│   │                 │ NO CREDENTIALS                │                    │  │
│   │                 ▼                               │                    │  │
│   │          ┌─────────────┐                        │                    │  │
│   │          │ MCP Response│ ◄──────────────────────┘                    │  │
│   │          │ (data only) │                                             │  │
│   │          └──────┬──────┘                                             │  │
│   │                 │                                                    │  │
│   └─────────────────┼────────────────────────────────────────────────────┘  │
│                     │ stdio (local process, no network)                     │
│                     ▼                                                       │
│              ┌─────────────┐                                                │
│              │Claude Desktop│                                               │
│              │ / MCP Client │                                               │
│              └──────┬──────┘                                                │
│                     │                                                       │
└─────────────────────┼───────────────────────────────────────────────────────┘
                      │
                      │ 🔒 TLS (handled by Anthropic/vendor)
                      ▼
               ┌─────────────┐
               │   AI VM     │
               │ (Claude,    │
               │  GPT, etc.) │
               └─────────────┘
```

**Key Security Properties:**
1. Credentials are loaded from config, used internally, and NEVER serialised to MCP responses
2. stdio transport = local process communication, no network between MCP server and client
3. Only git pack data (file contents, commits, branches) flows through MCP
4. Anthropic/vendor handles encryption between their client and AI VM

---

## Design Decisions (Locked In)

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Config hot-reload | ❌ No | Security: config changes require restart to prevent runtime injection |
| Concurrent operations | ✅ Yes | Allow multiple repos to be accessed simultaneously |
| Timeline priority | Security first | Take time to do it right, no rushing |
| Transport | stdio only (v1) | Simplest, most secure for local MCP clients |
| SSH keys | User manages | User sets up keys on PC, we reference path or use ssh-agent |
| Large repos | Chunked streaming | Progress callbacks, stream data in chunks |
| Git LFS | Defer to v1.1 | v1.0: detect & warn; v1.1+: implement support |
| Spelling | British 🇬🇧 | colour, behaviour, organisation, centre, licence — it's posh! |
| Feature tracking | `.claude/features.json` | JSON format discourages inappropriate edits |

---

## Phase 0: Project Setup ✅ COMPLETE

- [x] Devcontainer configuration
- [x] Cargo.toml with dependencies
- [x] CI workflow (fmt, clippy, build, test)
- [x] VS Code configuration
- [x] GitHub Actions with restricted permissions
- [x] CodeQL analysis enabled

---

## Phase 0.5: Open Source Best Practices ✅ COMPLETE

- [x] `SECURITY.md` — Vulnerability reporting policy
- [x] `CONTRIBUTING.md` — Contributor guidelines
- [x] `CHANGELOG.md` — Keep a Changelog format
- [x] `CODE_OF_CONDUCT.md` — Contributor Covenant
- [x] Issue templates (bug report, feature request)
- [x] PR template with security checklist
- [x] `.claude/CLAUDE.md` — AI assistant context (Claude Code format)
- [x] Secret Scanning enabled
- [x] Push Protection enabled
- [x] Branch protection on `main` (PRs required, CI must pass, CodeQL required)
- [x] Tag protection (restrict create/update/delete, block force push)

### Repository Security Summary

| Protection | Status |
|------------|--------|
| Branch protection (`main`) | ✅ |
| Tag protection | ✅ |
| Secret scanning | ✅ |
| Push protection | ✅ |
| CodeQL analysis | ✅ |
| Community standards | ✅ 100% |

---

## Phase 0.6: CI/CD Optimisation ✅ COMPLETE

Reduced CI time from ~8 minutes to ~2-3 minutes (on cache hit).

### Optimisations Applied

| Before | After | Improvement |
|--------|-------|-------------|
| No caching | `Swatinem/rust-cache@v2` | ~50-70% faster on cache hit |
| 5 separate jobs | 2 jobs (quick-checks + build) | Less job overhead |
| fmt → clippy → build → test | Combined into single build job | No redundant compilation |
| Cache saved on every run | PRs read-only, main saves | Faster PR validation |

### CI Architecture

```
quick-checks (ubuntu)     build (matrix: ubuntu, macos, windows)
├── fmt                   ├── clippy
└── docs                  ├── build (debug + release)
                          └── test
```

### Caching Strategy (StringWiggler Pattern)

- **PRs:** Read-only cache (`save-if: false`)
- **Main branch:** Save cache after merge
- **Cache key:** `v1-rust-{os}-{hash of Cargo.lock}`

This prevents cache pollution from PR branches while keeping cache fresh from main.

---

## Phase 1: Core Infrastructure ← CURRENT

### 1.1 Configuration System
- [ ] Define config file JSON schema
- [ ] Create `src/config/mod.rs`
- [ ] Create `src/config/settings.rs`
- [x] Create `config/example-config.json`

### 1.2 Credential Management
- [ ] Create `src/auth/mod.rs`
- [ ] Create `src/auth/credentials.rs`
- [ ] Create `src/auth/matcher.rs`
- [ ] Use `secrecy` crate for sensitive strings

### 1.3 Error Handling
- [ ] Create `src/error.rs` with custom error types

---

## Phase 2: MCP Server Implementation

- [ ] Create `src/mcp/mod.rs`
- [ ] Create `src/mcp/transport.rs`
- [ ] Create `src/mcp/server.rs`
- [ ] Implement MCP lifecycle

---

## Phase 3: Git Operations (via git2-rs)

- [ ] Clone, Pull, Push, Fetch operations
- [ ] Progress callbacks
- [ ] LFS detection

---

## Phase 4: Security & Safety

- [ ] Audit logging
- [ ] Safety guardrails (protected branches, force push blocking)
- [ ] Credential security verification

---

## Phase 5: AI Identity Management

- [ ] Configurable AI identity for commits

---

## Phase 6: CLI & UX

- [ ] CLI commands and flags
- [ ] Logging configuration

---

## Phase 7: Cross-Platform Release

- [ ] Build targets (Windows, macOS, Linux)
- [ ] Release workflow

---

## Phase 8: Documentation

- [x] `CONTRIBUTING.md`
- [x] `SECURITY.md`
- [x] `CHANGELOG.md`
- [ ] Expand `README.md`

---

## Phase 9: Testing

- [ ] Unit tests
- [ ] Integration tests
- [ ] Security tests
- [ ] Manual testing with MCP clients

---

## References

- **MCP Specification:** https://modelcontextprotocol.io/
- **git2-rs Documentation:** https://docs.rs/git2
- **Open Source Guides:** https://opensource.guide/
- **Claude Code Docs:** https://docs.anthropic.com/en/docs/claude-code
- **Swatinem/rust-cache:** https://github.com/Swatinem/rust-cache

---

*Last updated: 2025-12-28*
