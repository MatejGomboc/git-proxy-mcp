# TODO — Development Battle Plan

## Overview

**Goal:** Build a secure, AI-agnostic Git proxy MCP server in Rust that keeps credentials on the user's machine while allowing AI assistants to work with repos in their own environments.

**Guiding Principle:** Security over speed. Take the time to do it right.

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
1. Credentials are loaded from config, used internally, and NEVER serialized to MCP responses
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

---

## Phase 0: Project Setup

### 0.1 Development Environment
- [ ] Create `.devcontainer/devcontainer.json` for VS Code / Codespaces / CI
- [ ] Create `.devcontainer/Dockerfile` with Rust toolchain
- [ ] Include: rustc, cargo, clippy, rustfmt, rust-analyzer, git, OpenSSL dev libs
- [ ] Test devcontainer works in VS Code and GitHub Codespaces

### 0.2 Build Configuration
- [ ] Create `Cargo.toml` with dependencies
- [ ] Create `src/main.rs` skeleton
- [ ] Create `.gitignore` for Rust
- [ ] Add `rust-toolchain.toml` for consistent Rust version (stable)
- [ ] Create `rustfmt.toml` with formatting rules
- [ ] Create `clippy.toml` or configure in `Cargo.toml`

### 0.3 CI/CD Workflows
- [ ] Update `.github/workflows/ci_pr_validation.yml` for Rust:
  - [ ] `cargo fmt --check`
  - [ ] `cargo clippy -- -D warnings`
  - [ ] `cargo build`
  - [ ] `cargo test`
  - [ ] Matrix: ubuntu, macos, windows
- [ ] Add `.github/workflows/release.yml` for cross-platform binary releases

### 0.4 VS Code Configuration
- [ ] Update `.vscode/extensions.json` for Rust (rust-analyzer, etc.)
- [ ] Update `.vscode/settings.json` for Rust

### 0.5 Verification
- [ ] Verify project builds locally
- [ ] Verify CI passes on all platforms
- [ ] Verify devcontainer works

---

## Phase 1: Core Infrastructure

### 1.1 Configuration System
- [ ] Define config file JSON schema
- [ ] Create `src/config/mod.rs`
- [ ] Create `src/config/settings.rs` — parse and validate config
- [ ] Support platform-specific default paths:
  - Linux: `~/.config/git-proxy-mcp/config.json`
  - macOS: `~/Library/Application Support/git-proxy-mcp/config.json`
  - Windows: `%APPDATA%\git-proxy-mcp\config.json`
- [ ] Create `config/example-config.json` as reference
- [ ] Config is loaded ONCE at startup (no hot-reload for security)

### 1.2 Credential Management
- [ ] Create `src/auth/mod.rs`
- [ ] Create `src/auth/credentials.rs` — load credentials from config
- [ ] Create `src/auth/matcher.rs` — match URL to credential entry (glob patterns)
- [ ] Support auth methods:
  - [ ] PAT (Personal Access Token) — HTTPS
  - [ ] SSH key (file path, optional passphrase prompt at startup)
  - [ ] Basic auth (username + password/token)
- [ ] Use `secrecy` crate for sensitive strings (zeroize on drop)
- [ ] **Credentials NEVER appear in**:
  - MCP responses
  - Log messages
  - Error messages
  - Debug output

### 1.3 Error Handling
- [ ] Create `src/error.rs` with custom error types
- [ ] Use `thiserror` for error definitions
- [ ] Use `anyhow` for error propagation
- [ ] Scrub any potential credential data from error chains

---

## Phase 2: MCP Server Implementation

### 2.1 MCP Protocol Core
- [ ] Create `src/mcp/mod.rs`
- [ ] Create `src/mcp/transport.rs` — stdio JSON-RPC transport (stdin/stdout)
- [ ] Create `src/mcp/server.rs` — request/response handling
- [ ] Create `src/mcp/schema.rs` — tool definitions
- [ ] Implement MCP lifecycle:
  - [ ] `initialize` / `initialized`
  - [ ] `tools/list`
  - [ ] `tools/call`
  - [ ] `shutdown`
- [ ] Support MCP protocol version negotiation
- [ ] Handle concurrent tool calls (async/await with Tokio)

### 2.2 Tool Definitions
Define these MCP tools:

| Tool | Description | Priority |
|------|-------------|----------|
| `list_remotes` | List configured remotes (names only, no secrets) | P0 |
| `clone` | Clone repo via proxy, stream to caller | P0 |
| `pull` | Pull latest changes | P0 |
| `push` | Push commits | P0 |
| `fetch` | Fetch without merge | P1 |
| `list_remote_branches` | List branches on remote | P1 |
| `list_remote_tags` | List tags on remote | P2 |

---

## Phase 3: Git Operations (via git2-rs)

### 3.1 Git Proxy Core
- [ ] Create `src/git/mod.rs`
- [ ] Create `src/git/proxy.rs` — core proxy logic
- [ ] Create `src/git/callbacks.rs` — git2 credential callbacks
- [ ] Credential callback injects auth WITHOUT exposing it

### 3.2 Clone Operation
- [ ] Create `src/git/clone.rs`
- [ ] Accept: remote URL, destination path, optional branch, optional depth
- [ ] Match URL to credentials via `auth/matcher.rs`
- [ ] Inject credentials via git2 RemoteCallbacks
- [ ] Stream progress back to MCP client
- [ ] Return: success/failure, final path, branch info, commit hash

### 3.3 Pull Operation
- [ ] Create `src/git/pull.rs`
- [ ] Accept: local repo path, optional remote name, optional branch
- [ ] Fetch + fast-forward merge
- [ ] Handle merge conflicts gracefully (report, don't auto-resolve)
- [ ] Return: success/failure, updated refs, conflict info if any

### 3.4 Push Operation
- [ ] Create `src/git/push.rs`
- [ ] Accept: local repo path, remote name, branch, optional force flag
- [ ] Inject credentials for push authentication
- [ ] Handle rejection gracefully (non-fast-forward, permissions)
- [ ] Return: success/failure, pushed refs, rejection reason if any

### 3.5 Fetch Operation
- [ ] Create `src/git/fetch.rs`
- [ ] Fetch only, no merge
- [ ] Return: list of updated refs with old/new commit hashes

### 3.6 Remote Info Operations
- [ ] Create `src/git/remote_info.rs`
- [ ] `list_remote_branches`: ls-remote for branches
- [ ] `list_remote_tags`: ls-remote for tags

---

## Phase 4: Security & Safety

### 4.1 Audit Logging
- [ ] Create `src/security/mod.rs`
- [ ] Create `src/security/audit.rs`
- [ ] Log all git operations with timestamp
- [ ] Log: operation type, repo URL (host/org/repo only), success/failure, duration
- [ ] **NEVER log**: full URLs with tokens, credentials, auth headers
- [ ] Configurable log path and retention
- [ ] Default: `~/.config/git-proxy-mcp/audit.log`

### 4.2 Safety Guardrails
- [ ] Create `src/security/policy.rs`
- [ ] Configurable options:
  - [ ] `allow_force_push: bool` (default: false)
  - [ ] `protected_branches: Vec<String>` — block push to these (default: ["main", "master"])
  - [ ] `repo_allowlist: Option<Vec<String>>` — glob patterns, if set only these repos allowed
  - [ ] `repo_blocklist: Option<Vec<String>>` — glob patterns, these repos always blocked
- [ ] Validate all operations against policy BEFORE executing

### 4.3 Credential Security
- [ ] Use `secrecy::SecretString` for all credential storage
- [ ] Credentials zeroized on drop (secrecy crate handles this)
- [ ] No `Debug` impl that could leak credentials
- [ ] No `Display` impl that could leak credentials
- [ ] Review all error paths for potential credential leakage

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
- [ ] Configure git2 signature for commits made via proxy
- [ ] This distinguishes AI commits from human commits in git history
- [ ] Clear attribution in `git log` and `git blame`

---

## Phase 6: CLI & UX

### 6.1 Command Line Interface
- [ ] Use `clap` for argument parsing
- [ ] Commands:
  - [ ] `git-proxy-mcp` (no subcommand) — run as MCP server (default)
  - [ ] `git-proxy-mcp check-config` — validate config file
  - [ ] `git-proxy-mcp init-config` — create example config interactively
  - [ ] `git-proxy-mcp --version` — show version info
  - [ ] `git-proxy-mcp --help` — show help
- [ ] Flags:
  - [ ] `--config <path>` — custom config path
  - [ ] `--verbose` / `-v` — verbose logging (to stderr)
  - [ ] `--quiet` / `-q` — minimal output

### 6.2 Logging
- [ ] Use `tracing` + `tracing-subscriber`
- [ ] Log to stderr ONLY (stdout reserved for MCP JSON-RPC)
- [ ] Configurable log level via CLI and config
- [ ] Default: `warn` level

---

## Phase 7: Cross-Platform Release

### 7.1 Build Targets
- [ ] `x86_64-pc-windows-msvc` — Windows 64-bit
- [ ] `x86_64-apple-darwin` — macOS Intel
- [ ] `aarch64-apple-darwin` — macOS Apple Silicon
- [ ] `x86_64-unknown-linux-gnu` — Linux 64-bit
- [ ] `aarch64-unknown-linux-gnu` — Linux ARM64 (Raspberry Pi, etc.)

### 7.2 Release Workflow
- [ ] GitHub Actions workflow triggered on tag push (`v*`)
- [ ] Build binaries for all targets (use cross-compilation or matrix)
- [ ] Create GitHub Release with attached binaries
- [ ] Generate checksums (SHA256)
- [ ] Sign releases (optional, future)

### 7.3 Binary Optimization
- [ ] Enable LTO in release profile
- [ ] Strip symbols
- [ ] Consider `opt-level = "z"` for size if binary is too large

---

## Phase 8: Documentation

- [ ] Expand `README.md`:
  - [ ] Project description and motivation
  - [ ] Security architecture explanation
  - [ ] Installation instructions (download binary)
  - [ ] Configuration guide with examples
  - [ ] MCP client setup examples (Claude Desktop, VS Code, Cursor, etc.)
  - [ ] Security considerations and threat model
  - [ ] Contributing guidelines
- [ ] Add `CONTRIBUTING.md`
- [ ] Add `SECURITY.md` — how to report vulnerabilities
- [ ] Add `CHANGELOG.md` (keep-a-changelog format)
- [ ] Inline rustdoc comments for public APIs
- [ ] Generate and host rustdoc (GitHub Pages?)

---

## Phase 9: Testing

### 9.1 Unit Tests
- [ ] Config parsing tests (valid, invalid, missing fields)
- [ ] URL-to-credential matching tests (globs, edge cases)
- [ ] Policy validation tests (allow/block scenarios)
- [ ] Error handling tests (no credential leaks)
- [ ] Credential scrubbing tests

### 9.2 Integration Tests
- [ ] MCP protocol compliance tests (initialize, tools/list, tools/call)
- [ ] JSON-RPC message format tests
- [ ] Git operations against test repos (use temp repos)
- [ ] End-to-end: clone → modify → commit → push (mock server or real test repo)

### 9.3 Security Tests
- [ ] Verify credentials don't appear in any output
- [ ] Verify audit log doesn't contain credentials
- [ ] Verify error messages don't leak credentials
- [ ] Fuzz config parsing

### 9.4 Manual Testing
- [ ] Test with Claude Desktop
- [ ] Test with VS Code + Continue.dev
- [ ] Test with Cursor
- [ ] Test on Windows, macOS, Linux
- [ ] Test with GitHub, GitLab, Bitbucket

---

## Future Ideas (Post v1.0)

- [ ] OS keychain integration for credential storage (keyring crate)
- [ ] Encrypted config file option
- [ ] Sparse checkout support (clone only specific directories)
- [ ] Git LFS support
- [ ] Submodule support
- [ ] PR/MR creation integration (GitHub/GitLab APIs)
- [ ] Streamable HTTP transport (for remote MCP scenarios)
- [ ] Web UI for config management
- [ ] Package managers: Homebrew, apt, winget, cargo install

---

## Dependencies (Cargo.toml)

```toml
[package]
name = "git-proxy-mcp"
version = "0.1.0"
edition = "2021"
license = "GPL-3.0-or-later"
description = "Secure Git proxy MCP server for AI assistants"
repository = "https://github.com/MatejGomboc/git-proxy-mcp"
keywords = ["mcp", "git", "proxy", "ai", "llm"]
categories = ["development-tools"]

[dependencies]
# Git operations
git2 = "0.19"

# Async runtime
tokio = { version = "1", features = ["full"] }

# Serialization
serde = { version = "1", features = ["derive"] }
serde_json = "1"

# CLI
clap = { version = "4", features = ["derive"] }

# Logging
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }

# Error handling
thiserror = "1"
anyhow = "1"

# Security
secrecy = { version = "0.10", features = ["serde"] }

# URL parsing
url = "2"

# Glob matching for repo allowlists
glob = "0.3"

# Platform-specific directories
dirs = "5"

[dev-dependencies]
tempfile = "3"

[profile.release]
opt-level = 3
lto = true
strip = true
panic = "abort"
```

---

## Devcontainer Setup

`.devcontainer/devcontainer.json`:
```json
{
  "name": "git-proxy-mcp",
  "build": {
    "dockerfile": "Dockerfile"
  },
  "features": {
    "ghcr.io/devcontainers/features/rust:1": {
      "version": "latest",
      "profile": "default"
    },
    "ghcr.io/devcontainers/features/git:1": {}
  },
  "customizations": {
    "vscode": {
      "extensions": [
        "rust-lang.rust-analyzer",
        "tamasfe.even-better-toml",
        "serayuzgur.crates",
        "vadimcn.vscode-lldb",
        "usernamehw.errorlens",
        "EditorConfig.EditorConfig"
      ],
      "settings": {
        "rust-analyzer.check.command": "clippy"
      }
    }
  },
  "postCreateCommand": "cargo fetch",
  "remoteUser": "vscode"
}
```

---

## Open Questions

1. **SSH key passphrase handling** — Prompt at startup? Require unencrypted keys? Use ssh-agent?
2. **Large repo handling** — Memory limits? Stream in chunks? Progress reporting?
3. **Git LFS** — Support in v1.0 or defer to future?

---

## Resolved Questions

| Question | Decision |
|----------|----------|
| Config hot-reload? | No — security concern, require restart |
| Concurrent operations? | Yes — allow multiple repos simultaneously |
| Timeline? | Security over speed, no rushing |
| Devcontainer? | Yes — for VS Code, CI/CD, Codespaces |

---

*Last updated: 2025-12-28*
