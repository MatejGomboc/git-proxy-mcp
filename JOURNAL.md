# Development Journal

This file tracks the development progress of `git-proxy-mcp`. It serves as a handoff document between Claude instances so each new instance knows what was done and what to do next.

---

## How to Use This Journal

**For Claude instances:** Read this file first to understand the current state, then continue from "Current Status".

**For humans:** This is an internal dev log. See README.md for user documentation.

---

## Project Overview

**git-proxy-mcp** is a secure Git proxy MCP server that:
- Keeps credentials on the user's PC (never transmitted)
- Allows AI assistants to clone/pull/push to private repos
- Works with any MCP-compatible AI client (Claude, GPT, etc.)
- Written in Rust for security and performance

See `TODO.md` for the full battle plan.

---

## Current Status

### Phase: 0 — Project Setup (NOT STARTED)

**Next steps:**
1. Create `.devcontainer/` for VS Code / Codespaces / CI
2. Create `Cargo.toml` with dependencies
3. Create `src/main.rs` skeleton ("Hello World")
4. Create `.gitignore` for Rust
5. Update CI workflow for Rust (currently configured for C++ ARM project)
6. Update VS Code extensions for Rust

**Blockers:** None

**Notes:** The existing CI workflow (`.github/workflows/ci_pr_validation.yml`) is from another project and needs to be replaced with Rust-specific steps.

---

## Session Log

### 2025-12-28 — Initial Planning (Claude + MatejGomboc)

**What happened:**
- Discussed the concept of a Git proxy MCP server
- Designed the security architecture (credentials never leave user's PC)
- Chose Rust as the implementation language
- Chose GPL v3 as the license
- Created comprehensive `TODO.md` with battle plan
- Resolved key design decisions:
  - No config hot-reload (security)
  - Yes concurrent operations
  - SSH keys: user manages, we reference
  - Large repos: chunked streaming
  - Git LFS: defer to v1.1

**Key files created:**
- `TODO.md` — Full development battle plan

**Decisions made:**
| Decision | Choice |
|----------|--------|
| Language | Rust |
| License | GPL v3 |
| MCP Transport | stdio (v1.0) |
| Config reload | No (security) |
| Concurrency | Yes |
| Git LFS | v1.1 (detect & warn in v1.0) |

**Next session should:**
1. Start Phase 0: Project Setup
2. Create devcontainer
3. Create Cargo.toml and src/main.rs
4. Update CI workflow for Rust
5. Get "Hello World" compiling on all platforms

---

## Architecture Notes

### Security Model

```
Credentials: config.json → Auth Module → GitHub API
                              ↓
                         (internally)
                              ↓
Git Data: GitHub API → git2-rs → MCP Response → AI VM

Credentials NEVER appear in MCP responses.
```

### Key Crates

| Crate | Purpose |
|-------|---------|
| `git2` | Git operations (libgit2 bindings) |
| `tokio` | Async runtime |
| `serde` / `serde_json` | Config parsing, MCP JSON-RPC |
| `clap` | CLI argument parsing |
| `tracing` | Logging (to stderr only) |
| `secrecy` | Secure credential handling (zeroize on drop) |
| `thiserror` / `anyhow` | Error handling |

### MCP Tools (v1.0)

| Tool | Priority |
|------|----------|
| `list_remotes` | P0 |
| `clone` | P0 |
| `pull` | P0 |
| `push` | P0 |
| `fetch` | P1 |
| `list_remote_branches` | P1 |
| `list_remote_tags` | P2 |

---

## File Structure (Planned)

```
git-proxy-mcp/
├── .devcontainer/
│   ├── devcontainer.json
│   └── Dockerfile
├── .github/
│   └── workflows/
│       ├── ci_pr_validation.yml
│       └── release.yml
├── .vscode/
│   ├── extensions.json
│   └── settings.json
├── config/
│   └── example-config.json
├── src/
│   ├── main.rs
│   ├── error.rs
│   ├── auth/
│   │   ├── mod.rs
│   │   ├── credentials.rs
│   │   └── matcher.rs
│   ├── config/
│   │   ├── mod.rs
│   │   └── settings.rs
│   ├── git/
│   │   ├── mod.rs
│   │   ├── proxy.rs
│   │   ├── callbacks.rs
│   │   ├── clone.rs
│   │   ├── pull.rs
│   │   ├── push.rs
│   │   ├── fetch.rs
│   │   ├── remote_info.rs
│   │   └── lfs.rs
│   ├── mcp/
│   │   ├── mod.rs
│   │   ├── server.rs
│   │   ├── transport.rs
│   │   └── schema.rs
│   └── security/
│       ├── mod.rs
│       ├── audit.rs
│       └── policy.rs
├── tests/
├── Cargo.toml
├── Cargo.lock
├── rust-toolchain.toml
├── rustfmt.toml
├── .gitignore
├── .editorconfig
├── LICENSE
├── README.md
├── TODO.md
├── JOURNAL.md
├── CHANGELOG.md
├── CONTRIBUTING.md
└── SECURITY.md
```

---

## Tips for Future Claude Instances

1. **Always read `TODO.md` first** — it has the full battle plan with checkboxes
2. **Update this journal** after each session with what you did and what's next
3. **Security is paramount** — credentials must NEVER leak to logs, errors, or MCP responses
4. **Test on all platforms** — Windows, macOS, Linux
5. **Keep commits atomic** — one logical change per commit
6. **Use conventional commits** — `feat:`, `fix:`, `docs:`, `chore:`, etc.

---

## Links

- **Repo:** https://github.com/MatejGomboc/git-proxy-mcp
- **MCP Spec:** https://modelcontextprotocol.io
- **git2-rs:** https://docs.rs/git2
- **Rust MCP examples:** https://github.com/modelcontextprotocol/servers

---

*Last updated: 2025-12-28*
