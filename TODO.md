# TODO — Development Battle Plan

## Overview

**Goal:** Build a secure, AI-agnostic Git proxy MCP server in Rust that keeps credentials on the user's machine while allowing AI assistants to work with repos in their own environments.

**Guiding Principles:**
- Security over speed. Take the time to do it right.
- Work on ONE feature at a time. See `.claude/features.json` for tracking.
- Use British spelling in documentation and user-facing text. It's posh! 🇬🇧

**For AI Assistants:** See `.claude/INSTRUCTIONS.md` for quick start guide.

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

## Style Guidelines

### British Spelling 🇬🇧

Use British spelling throughout documentation and user-facing text.

| ❌ American | ✅ British |
|-------------|------------|
| color | colour |
| behavior | behaviour |
| organization | organisation |
| center | centre |
| license (noun) | licence |
| analyze | analyse |
| initialize | initialise |
| customize | customise |
| serialized | serialised |

**Note:** Code identifiers (variable names, function names) may use American spelling where it matches Rust/library conventions.

---

## Phase 0: Project Setup ✅

### 0.1 Development Environment
- [x] Create `.devcontainer/devcontainer.json` for VS Code / Codespaces / CI
- [x] Create `.devcontainer/Dockerfile` with Rust toolchain
- [x] Include: rustc, cargo, clippy, rustfmt, rust-analyzer, git, OpenSSL dev libs
- [ ] Test devcontainer works in VS Code and GitHub Codespaces

### 0.2 Build Configuration
- [x] Create `Cargo.toml` with dependencies
- [x] Create `src/main.rs` skeleton
- [x] Create `.gitignore` for Rust
- [x] Add `rust-toolchain.toml` for consistent Rust version (stable)
- [x] Create `rustfmt.toml` with formatting rules
- [x] Configure clippy lints in `Cargo.toml`

### 0.3 CI/CD Workflows
- [x] Update `.github/workflows/ci_pr_validation.yml` for Rust
- [ ] Add `.github/workflows/release.yml` for cross-platform binary releases

### 0.4 VS Code Configuration
- [x] Update `.vscode/extensions.json` for Rust (rust-analyzer, etc.)
- [x] Update `.vscode/settings.json` for Rust

### 0.5 GitHub Repository Settings
- [x] Enable GitHub Actions with restricted permissions
- [x] Enable CodeQL analysis
- [ ] Enable Secret Scanning (critical!)
- [ ] Enable Push Protection
- [ ] Set up branch protection rules for `main`

### 0.6 Verification
- [ ] Verify project builds locally
- [ ] Verify CI passes on all platforms
- [ ] Verify devcontainer works

---

## Phase 0.5: Open Source Best Practices ✅

### 0.5.1 Security Documentation
- [x] Create `SECURITY.md`

### 0.5.2 Community Documentation
- [ ] Create `CODE_OF_CONDUCT.md` *(owner to create — requires personal contact info)*
- [x] Create `CONTRIBUTING.md`

### 0.5.3 Issue & PR Templates
- [x] Create `.github/ISSUE_TEMPLATE/bug_report.md`
- [x] Create `.github/ISSUE_TEMPLATE/feature_request.md`
- [x] Create `.github/ISSUE_TEMPLATE/config.yml`
- [x] Create `.github/PULL_REQUEST_TEMPLATE.md`

### 0.5.4 Project Documentation
- [x] Create `CHANGELOG.md`
- [x] Create `.claude/` folder for AI assistant documentation
- [ ] Expand `README.md`

### Open Source Checklist Summary

| Item | Status |
|------|--------|
| LICENSE | ✅ Done |
| README.md | ⚠️ Basic |
| CONTRIBUTING.md | ✅ Done |
| CODE_OF_CONDUCT.md | ⏳ Owner |
| SECURITY.md | ✅ Done |
| CHANGELOG.md | ✅ Done |
| Issue/PR Templates | ✅ Done |
| .claude/ folder | ✅ Done |

---

## Phase 1: Core Infrastructure

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
- **Contributor Covenant:** https://www.contributor-covenant.org/

---

*Last updated: 2025-12-28*
