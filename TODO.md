# TODO — Development Battle Plan

## Overview

**Goal:** Build a secure, AI-agnostic Git proxy MCP server in Rust that keeps credentials on the user's machine while allowing AI assistants to work with repos in their own environments.

**Guiding Principles:**
- Security over speed. Take the time to do it right.
- Work on ONE feature at a time. See `features.json` for tracking.
- Use British spelling in documentation and user-facing text. It's posh! 🇬🇧

---

## Feature Tracking

Features are tracked in `features.json` with pass/fail status. 

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
| Feature tracking | `features.json` | JSON format discourages inappropriate edits |

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

## Phase 0: Project Setup

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
- [x] Update `.github/workflows/ci_pr_validation.yml` for Rust:
  - [x] `cargo fmt --check`
  - [x] `cargo clippy -- -D warnings`
  - [x] `cargo build`
  - [x] `cargo test`
  - [x] Matrix: ubuntu, macos, windows
- [ ] Add `.github/workflows/release.yml` for cross-platform binary releases

### 0.4 VS Code Configuration
- [x] Update `.vscode/extensions.json` for Rust (rust-analyzer, etc.)
- [x] Update `.vscode/settings.json` for Rust

### 0.5 GitHub Repository Settings
- [x] Enable GitHub Actions with restricted permissions
- [x] Add `dtolnay/rust-toolchain@stable` to allowed actions
- [x] Enable CodeQL analysis for Rust and GitHub Actions
- [x] Enable Copilot Autofix for CodeQL alerts
- [x] Configure code review limits
- [x] Set workflow permissions to read-only
- [ ] Enable Secret Scanning (critical for credential-handling project!)
- [ ] Enable Push Protection (block commits containing secrets)
- [ ] Enable Dependabot security updates
- [ ] Set up branch protection rules for `main`

### 0.6 Verification
- [ ] Verify project builds locally
- [ ] Verify CI passes on all platforms
- [ ] Verify devcontainer works

---

## Phase 0.5: Open Source Best Practices

Based on https://opensource.guide/ recommendations. Essential for building a welcoming, trustworthy open source project.

### 0.5.1 Security Documentation (Priority: CRITICAL)
- [x] Create `SECURITY.md` — **Critical for a credential-handling project!**
  - [x] Vulnerability disclosure policy (how to report privately)
  - [x] Security update policy
  - [x] Supported versions
  - [x] Security contact (email or GitHub Security Advisories)
  - [x] Response timeline expectations
  - [x] Link to GitHub's private vulnerability reporting

### 0.5.2 Community Documentation
- [ ] Create `CODE_OF_CONDUCT.md` *(owner to create — requires personal contact info)*
  - [ ] Adopt Contributor Covenant (https://www.contributor-covenant.org/)
  - [ ] Define enforcement contacts
  - [ ] Link from README and CONTRIBUTING
- [x] Create `CONTRIBUTING.md`
  - [x] How to report bugs
  - [x] How to suggest features
  - [x] How to submit pull requests
  - [x] Coding style guidelines (link to rustfmt.toml, clippy config)
  - [x] Commit message format (conventional commits)
  - [x] PR review process
  - [x] Testing requirements
  - [x] British spelling requirement for docs 🇬🇧

### 0.5.3 Issue & PR Templates
- [x] Create `.github/ISSUE_TEMPLATE/bug_report.md`
  - [x] Steps to reproduce
  - [x] Expected vs actual behaviour
  - [x] Environment (OS, version)
  - [x] Logs/error messages (remind: no credentials!)
- [x] Create `.github/ISSUE_TEMPLATE/feature_request.md`
  - [x] Problem description
  - [x] Proposed solution
  - [x] Alternatives considered
- [x] Create `.github/ISSUE_TEMPLATE/config.yml`
  - [x] Disable blank issues (require template)
  - [x] Link to discussions for questions
- [x] Create `.github/PULL_REQUEST_TEMPLATE.md`
  - [x] Description of changes
  - [x] Related issue(s)
  - [x] Checklist: tests, docs, changelog, no credentials in logs

### 0.5.4 Project Documentation
- [x] Create `CHANGELOG.md` (Keep a Changelog format)
  - [x] Document all notable changes
  - [x] Categories: Added, Changed, Deprecated, Removed, Fixed, Security
- [ ] Expand `README.md`
  - [ ] Clear project description
  - [ ] Security architecture diagram
  - [ ] Installation instructions
  - [ ] Quick start guide
  - [ ] Configuration reference
  - [ ] MCP client setup examples
  - [ ] Badges: CI status, license, version
  - [ ] Links to CONTRIBUTING, CODE_OF_CONDUCT, SECURITY

### 0.5.5 GitHub Repository Features
- [ ] Enable Discussions (for Q&A, ideas)
- [ ] Configure issue labels (bug, enhancement, security, good first issue, help wanted)
- [ ] Add repository topics (mcp, git, rust, security, ai, llm)
- [ ] Set repository description
- [ ] Add social preview image (optional)

### Open Source Checklist Summary

| Item | Status | Priority |
|------|--------|----------|
| LICENSE | ✅ Done | - |
| README.md | ⚠️ Basic | High |
| CONTRIBUTING.md | ✅ Done | - |
| CODE_OF_CONDUCT.md | ⏳ Owner to create | Medium |
| SECURITY.md | ✅ Done | - |
| CHANGELOG.md | ✅ Done | - |
| Issue Templates | ✅ Done | - |
| PR Template | ✅ Done | - |
| Branch Protection | ⏳ Pending | High |
| Secret Scanning | ❌ Disabled | **CRITICAL** |
| Push Protection | ❌ Disabled | **CRITICAL** |
| Dependabot | ⚠️ Alerts only | Medium |

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
- [x] Create `config/example-config.json` as reference
- [ ] Config is loaded ONCE at startup (no hot-reload for security)

### 1.2 Credential Management
- [ ] Create `src/auth/mod.rs`
- [ ] Create `src/auth/credentials.rs` — load credentials from config
- [ ] Create `src/auth/matcher.rs` — match URL to credential entry (glob patterns)
- [ ] Support auth methods:
  - [ ] PAT (Personal Access Token) — HTTPS
  - [ ] SSH key file path (user manages keys, we reference)
  - [ ] SSH agent (if available, use automatically)
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
  - [ ] `initialise` / `initialised`
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
- [ ] Progress callbacks for streaming/chunked operations

### 3.2 Clone Operation
- [ ] Create `src/git/clone.rs`
- [ ] Accept: remote URL, destination path, optional branch, optional depth
- [ ] Match URL to credentials via `auth/matcher.rs`
- [ ] Inject credentials via git2 RemoteCallbacks
- [ ] Stream progress back to MCP client (chunked)
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

### 3.7 LFS Detection (v1.0)
- [ ] Create `src/git/lfs.rs`
- [ ] Detect if repo uses LFS (check `.gitattributes`)
- [ ] Warn user: "This repo uses Git LFS. Large files are placeholders only."
- [ ] Clone proceeds — code files work, LFS files are pointer files

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
- [ ] Credentials zeroised on drop (secrecy crate handles this)
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

### 7.3 Binary Optimisation
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
- [x] Add `CONTRIBUTING.md`
- [x] Add `SECURITY.md` — how to report vulnerabilities
- [x] Add `CHANGELOG.md` (keep-a-changelog format)
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
- [ ] MCP protocol compliance tests (initialise, tools/list, tools/call)
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

## Version Roadmap

### v1.0 — Core Functionality
- Config, credentials, MCP server
- clone, pull, push, fetch operations
- Security guardrails, audit logging
- Cross-platform binaries

### v1.1 — Git LFS Support
- [ ] Shell out to `git-lfs` binary if available
- [ ] Or implement LFS protocol directly
- [ ] Automatic LFS pull after clone

### v1.2+ — Future
- [ ] OS keychain integration (keyring crate)
- [ ] Encrypted config file option
- [ ] Sparse checkout support
- [ ] Submodule support
- [ ] PR/MR creation integration
- [ ] Streamable HTTP transport
- [ ] Package managers: Homebrew, apt, winget

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

# Serialisation
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

## Resolved Questions

| Question | Decision | Rationale |
|----------|----------|-----------|
| Config hot-reload? | No | Security: prevent runtime injection |
| Concurrent operations? | Yes | Allow multiple repos simultaneously |
| Timeline? | Security first | No rushing, do it right |
| Devcontainer? | Yes | VS Code, CI/CD, Codespaces consistency |
| SSH keys? | User manages | We reference path or use ssh-agent |
| Large repos? | Chunked streaming | Progress callbacks, stream in chunks |
| Git LFS? | Defer to v1.1 | v1.0: detect & warn only |
| Spelling? | British 🇬🇧 | colour, behaviour, organisation — it's posh! |
| Feature tracking? | `features.json` | JSON format, per Anthropic guidance |

---

## References

- **Open Source Guides:** https://opensource.guide/
- **Contributor Covenant:** https://www.contributor-covenant.org/
- **Keep a Changelog:** https://keepachangelog.com/
- **Conventional Commits:** https://www.conventionalcommits.org/
- **MCP Specification:** https://modelcontextprotocol.io/
- **git2-rs Documentation:** https://docs.rs/git2

---

*Last updated: 2025-12-28*
