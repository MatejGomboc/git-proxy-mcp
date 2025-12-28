# TODO — Development Battle Plan

## Overview

**Goal:** Build a secure, AI-agnostic Git proxy MCP server in Rust that keeps credentials on the user's machine while allowing AI assistants to work with repos in their own environments.

---

## Phase 0: Project Setup

- [ ] Update `.github/workflows/ci_pr_validation.yml` for Rust (build, test, clippy, fmt)
- [ ] Add `.github/workflows/release.yml` for cross-platform binary releases
- [ ] Update `.vscode/extensions.json` for Rust development
- [ ] Update `.vscode/settings.json` for Rust
- [ ] Create `Cargo.toml` with dependencies
- [ ] Create `src/main.rs` skeleton
- [ ] Create `.gitignore` for Rust
- [ ] Add `rust-toolchain.toml` for consistent Rust version
- [ ] Verify project builds on all 3 platforms (CI)

---

## Phase 1: Core Infrastructure

### 1.1 Configuration System
- [ ] Define config file schema (`config.json`)
- [ ] Create `src/config/mod.rs`
- [ ] Create `src/config/settings.rs` — parse and validate config
- [ ] Support platform-specific default paths:
  - Linux: `~/.config/git-proxy-mcp/config.json`
  - macOS: `~/Library/Application Support/git-proxy-mcp/config.json`
  - Windows: `%APPDATA%\git-proxy-mcp\config.json`
- [ ] Create `config/example-config.json` as reference

### 1.2 Credential Management
- [ ] Create `src/auth/mod.rs`
- [ ] Create `src/auth/credentials.rs` — load credentials from config
- [ ] Create `src/auth/matcher.rs` — match URL to credential entry (glob patterns)
- [ ] Support auth methods:
  - [x] PAT (Personal Access Token) — HTTPS
  - [ ] SSH key (with optional passphrase)
  - [ ] Basic auth (username + password/token)
- [ ] **Credentials never leave this module as strings** — only used internally for git ops
- [ ] (Future) OS keychain integration via `keyring` crate

### 1.3 Error Handling
- [ ] Create `src/error.rs` with custom error types
- [ ] Use `thiserror` for error definitions
- [ ] Use `anyhow` for error propagation
- [ ] Ensure no credentials leak in error messages

---

## Phase 2: MCP Server Implementation

### 2.1 MCP Protocol Core
- [ ] Create `src/mcp/mod.rs`
- [ ] Create `src/mcp/transport.rs` — stdio JSON-RPC transport
- [ ] Create `src/mcp/server.rs` — request/response handling
- [ ] Create `src/mcp/schema.rs` — tool definitions
- [ ] Implement MCP lifecycle:
  - [ ] `initialize` / `initialized`
  - [ ] `tools/list`
  - [ ] `tools/call`
  - [ ] `shutdown`
- [ ] Support MCP protocol version negotiation

### 2.2 Tool Definitions
Define these MCP tools:

| Tool | Description | Priority |
|------|-------------|----------|
| `list_remotes` | List configured remotes (names only, no secrets) | P0 |
| `clone` | Clone repo via proxy, stream to caller | P0 |
| `pull` | Pull latest changes | P0 |
| `push` | Push commits | P0 |
| `fetch` | Fetch without merge | P1 |
| `get_branches` | List remote branches | P1 |
| `get_tags` | List remote tags | P2 |

---

## Phase 3: Git Operations (via git2-rs)

### 3.1 Git Proxy Core
- [ ] Create `src/git/mod.rs`
- [ ] Create `src/git/proxy.rs` — core proxy logic
- [ ] Create `src/git/callbacks.rs` — git2 credential callbacks

### 3.2 Clone Operation
- [ ] Create `src/git/clone.rs`
- [ ] Accept: remote URL, destination path, optional branch, optional depth
- [ ] Match URL to credentials via `auth/matcher.rs`
- [ ] Inject credentials via git2 callbacks (never exposed)
- [ ] Stream progress back to MCP client
- [ ] Return: success/failure, final path, branch info

### 3.3 Pull Operation
- [ ] Create `src/git/pull.rs`
- [ ] Accept: local repo path, optional remote name, optional branch
- [ ] Fetch + merge (or fast-forward)
- [ ] Handle merge conflicts gracefully (report, don't auto-resolve)

### 3.4 Push Operation
- [ ] Create `src/git/push.rs`
- [ ] Accept: local repo path, remote name, branch
- [ ] Inject credentials for push authentication
- [ ] Handle rejection gracefully (non-fast-forward, permissions)

### 3.5 Fetch Operation
- [ ] Create `src/git/fetch.rs`
- [ ] Fetch only, no merge
- [ ] Return list of updated refs

---

## Phase 4: Security & Safety

### 4.1 Audit Logging
- [ ] Create `src/security/mod.rs`
- [ ] Create `src/security/audit.rs`
- [ ] Log all git operations with timestamp
- [ ] Log: operation type, repo URL (sanitized), success/failure
- [ ] **Never log credentials**
- [ ] Configurable log path and retention

### 4.2 Safety Guardrails
- [ ] Create `src/security/policy.rs`
- [ ] Configurable options:
  - [ ] `allow_force_push: bool`
  - [ ] `protected_branches: Vec<String>` — block push to these
  - [ ] `repo_allowlist: Option<Vec<String>>` — glob patterns
  - [ ] `repo_blocklist: Option<Vec<String>>` — glob patterns
- [ ] Validate all operations against policy before executing

### 4.3 Credential Security
- [ ] Credentials only held in memory during operation
- [ ] Zero-copy where possible (use `secrecy` crate?)
- [ ] Clear sensitive memory after use
- [ ] No credentials in logs, errors, or MCP responses

---

## Phase 5: AI Identity Management

- [ ] Add `ai_identity` section to config:
  ```json
  {
    "ai_identity": {
      "name": "AI Assistant",
      "email": "ai-assistant@noreply.local"
    }
  }
  ```
- [ ] Set `GIT_AUTHOR_NAME`, `GIT_AUTHOR_EMAIL` for commits made via proxy
- [ ] Set `GIT_COMMITTER_NAME`, `GIT_COMMITTER_EMAIL`
- [ ] This distinguishes AI commits from human commits in git history

---

## Phase 6: CLI & UX

### 6.1 Command Line Interface
- [ ] Use `clap` for argument parsing
- [ ] Commands:
  - [ ] `git-proxy-mcp serve` — run as MCP server (default)
  - [ ] `git-proxy-mcp check-config` — validate config file
  - [ ] `git-proxy-mcp init-config` — create example config
  - [ ] `git-proxy-mcp version` — show version info
- [ ] Flags:
  - [ ] `--config <path>` — custom config path
  - [ ] `--verbose` / `-v` — verbose logging
  - [ ] `--quiet` / `-q` — minimal output

### 6.2 Logging
- [ ] Use `tracing` + `tracing-subscriber`
- [ ] Log to stderr (stdout reserved for MCP JSON-RPC)
- [ ] Configurable log level via CLI and config

---

## Phase 7: Cross-Platform Release

### 7.1 Build Targets
- [ ] `x86_64-pc-windows-msvc` — Windows 64-bit
- [ ] `x86_64-apple-darwin` — macOS Intel
- [ ] `aarch64-apple-darwin` — macOS Apple Silicon
- [ ] `x86_64-unknown-linux-gnu` — Linux 64-bit
- [ ] (Optional) `aarch64-unknown-linux-gnu` — Linux ARM64

### 7.2 Release Workflow
- [ ] GitHub Actions workflow triggered on tag push (`v*`)
- [ ] Build binaries for all targets
- [ ] Create GitHub Release with attached binaries
- [ ] Generate checksums (SHA256)

### 7.3 Binary Optimization
- [ ] Enable LTO in release profile
- [ ] Strip symbols
- [ ] Target small binary size

---

## Phase 8: Documentation

- [ ] Expand `README.md`:
  - [ ] Project description and motivation
  - [ ] Installation instructions (download binary)
  - [ ] Configuration guide
  - [ ] MCP client setup examples (Claude Desktop, VS Code, etc.)
  - [ ] Security considerations
  - [ ] Contributing guidelines
- [ ] Add `CONTRIBUTING.md`
- [ ] Add `SECURITY.md` — how to report vulnerabilities
- [ ] Add `CHANGELOG.md`
- [ ] Inline rustdoc comments for public APIs

---

## Phase 9: Testing

### 9.1 Unit Tests
- [ ] Config parsing tests
- [ ] URL-to-credential matching tests
- [ ] Policy validation tests
- [ ] Error handling tests

### 9.2 Integration Tests
- [ ] MCP protocol compliance tests
- [ ] Git operations against test repos
- [ ] End-to-end clone/pull/push tests (with mock git server?)

### 9.3 Manual Testing
- [ ] Test with Claude Desktop
- [ ] Test with VS Code + Continue.dev
- [ ] Test with Cursor
- [ ] Test on Windows, macOS, Linux

---

## Future Ideas (Post v1.0)

- [ ] OS keychain integration for credential storage
- [ ] Encrypted config file option
- [ ] Sparse checkout support (clone only specific directories)
- [ ] LFS support
- [ ] Submodule support
- [ ] PR creation integration (GitHub/GitLab APIs)
- [ ] Web UI for config management
- [ ] Homebrew formula / apt repo / winget package

---

## Dependencies (Cargo.toml)

```toml
[dependencies]
git2 = "0.19"                    # Git operations
tokio = { version = "1", features = ["full"] }  # Async runtime
serde = { version = "1", features = ["derive"] }
serde_json = "1"
clap = { version = "4", features = ["derive"] }
tracing = "0.1"
tracing-subscriber = "0.3"
thiserror = "1"
anyhow = "1"
url = "2"
glob = "0.3"
dirs = "5"                       # Platform-specific directories

[dev-dependencies]
tempfile = "3"                   # Temp dirs for tests

[profile.release]
opt-level = 3
lto = true
strip = true
```

---

## Milestones

| Milestone | Target | Status |
|-----------|--------|--------|
| M0: Project compiles | Week 1 | 🔲 |
| M1: Config + MCP skeleton | Week 2 | 🔲 |
| M2: Clone works | Week 3 | 🔲 |
| M3: Pull + Push work | Week 4 | 🔲 |
| M4: Security + polish | Week 5 | 🔲 |
| M5: Release binaries | Week 6 | 🔲 |
| v1.0 | Week 6 | 🔲 |

---

## Open Questions

1. **SSH key passphrase handling** — prompt user? Or require unencrypted keys?
2. **Large file handling** — stream in chunks? Memory limits?
3. **Concurrent operations** — allow multiple clones at once?
4. **Config hot-reload** — watch for changes or require restart?

---

*Last updated: 2025-01-XX*
