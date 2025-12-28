# Development Journal

This file tracks the development progress of `git-proxy-mcp`. It serves as a handoff document between Claude instances so each new instance knows what was done and what to do next.

---

## Quick Reference

**Key files:**
- `.claude/INSTRUCTIONS.md` — AI assistant guidelines
- `.claude/JOURNAL.md` — This file (development history)
- `.claude/features.json` — Feature tracking
- `TODO.md` — Full battle plan (in repo root)

**At session end:**
1. Commit changes with conventional commit messages
2. Update this journal with what you did
3. Update `features.json` if you completed features

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

### Phase: 0.5 — Open Source Best Practices (NEARLY COMPLETE)

**Completed:**
- ✅ Phase 0 setup complete (devcontainer, Cargo.toml, CI, etc.)
- ✅ `SECURITY.md` created with comprehensive security policy
- ✅ `CONTRIBUTING.md` created with contributor guidelines
- ✅ `CHANGELOG.md` created (Keep a Changelog format)
- ✅ `.github/ISSUE_TEMPLATE/bug_report.md` created
- ✅ `.github/ISSUE_TEMPLATE/feature_request.md` created
- ✅ `.github/ISSUE_TEMPLATE/config.yml` created
- ✅ `.github/PULL_REQUEST_TEMPLATE.md` created
- ✅ `.claude/` folder created for AI agent documentation

**Pending (owner action required):**
- ⏳ `CODE_OF_CONDUCT.md` — Owner to create (requires personal enforcement contact)
- ⏳ Enable Secret Scanning in repo settings
- ⏳ Enable Push Protection in repo settings
- ⏳ Set up branch protection rules for `main`
- ⏳ Enable Discussions
- ⏳ Expand README.md with full documentation

**Next steps:**
1. Owner creates CODE_OF_CONDUCT.md with personal contact info
2. Enable Secret Scanning and Push Protection in GitHub settings
3. Set up branch protection for `main`
4. Begin Phase 1: Core Infrastructure (config system, credentials)

**Blockers:** None

---

## Session Log

### 2025-12-28 — Reorganise AI Documentation (Claude + MatejGomboc)

**What happened:**
- Created `.claude/` folder for AI-specific documentation
- Moved AI helper files to `.claude/`:
  - `JOURNAL.md` → `.claude/JOURNAL.md`
  - `features.json` → `.claude/features.json`
- Created `.claude/INSTRUCTIONS.md` as entry point for AI agents
- Kept `TODO.md` in root (useful for human contributors too)

**Files created:**
- `.claude/INSTRUCTIONS.md` — AI assistant quick start and guidelines
- `.claude/JOURNAL.md` — This file (moved from root)
- `.claude/features.json` — Feature tracking (moved from root)

**Files deleted:**
- `JOURNAL.md` (moved to .claude/)
- `features.json` (moved to .claude/)

---

### 2025-12-28 — Phase 0.5 Documentation Completion (Claude + MatejGomboc)

**What happened:**
- Continued from previous Claude instance that was cut off
- Created all remaining open source documentation files
- Skipped CODE_OF_CONDUCT.md (requires owner's personal contact info for enforcement)

**Files created:**
- `CONTRIBUTING.md` — Comprehensive contributor guidelines
- `CHANGELOG.md` — Keep a Changelog format
- `.github/ISSUE_TEMPLATE/bug_report.md` — Bug report template
- `.github/ISSUE_TEMPLATE/feature_request.md` — Feature request template
- `.github/ISSUE_TEMPLATE/config.yml` — Issue template configuration
- `.github/PULL_REQUEST_TEMPLATE.md` — PR template with security checklist

**Note on CODE_OF_CONDUCT.md:**
Skipped because it requires personal contact information for enforcement. Owner should create using Contributor Covenant template.

---

### 2025-12-28 — SECURITY.md Created (Claude + MatejGomboc)

**What happened:**
- Created comprehensive SECURITY.md with vulnerability disclosure policy

---

### 2025-12-28 — Repository Configuration & CI Fixes (Claude + MatejGomboc)

**What happened:**
- Configured GitHub repository settings (Actions, CodeQL, security)
- Fixed CI failures caused by clippy lints

---

### 2025-12-28 — Phase 0 Implementation (Claude + MatejGomboc)

**What happened:**
- Created devcontainer, Cargo.toml, main.rs skeleton
- Set up CI workflow for Rust
- Updated VS Code configuration

---

### 2025-12-28 — Initial Planning (Claude + MatejGomboc)

**What happened:**
- Designed security architecture
- Created TODO.md battle plan
- Made key design decisions

**Decisions made:**
| Decision | Choice |
|----------|--------|
| Language | Rust |
| Licence | GPL v3 |
| MCP Transport | stdio (v1.0) |
| Config reload | No (security) |
| Spelling | British 🇬🇧 |

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

## File Structure

```
git-proxy-mcp/
├── .claude/                    # AI assistant documentation
│   ├── INSTRUCTIONS.md         # Quick start for AI agents
│   ├── JOURNAL.md              # This file
│   └── features.json           # Feature tracking
├── .devcontainer/              # Dev environment
├── .github/                    # GitHub config
│   ├── ISSUE_TEMPLATE/
│   ├── PULL_REQUEST_TEMPLATE.md
│   └── workflows/
├── .vscode/                    # VS Code config
├── config/
│   └── example-config.json
├── src/
│   └── main.rs
├── Cargo.toml
├── CHANGELOG.md
├── CONTRIBUTING.md
├── LICENCE
├── README.md
├── SECURITY.md
└── TODO.md                     # Battle plan
```

---

## Tips for Future Claude Instances

1. **Read `.claude/INSTRUCTIONS.md`** first
2. **Work on ONE feature at a time**
3. **Update this journal** after each session
4. **Security is paramount** — credentials must NEVER leak
5. **Use British spelling** in docs 🇬🇧
6. **Use conventional commits**
7. **Leave codebase clean** — no broken builds

---

## Links

- **Repo:** https://github.com/MatejGomboc/git-proxy-mcp
- **MCP Spec:** https://modelcontextprotocol.io
- **git2-rs:** https://docs.rs/git2

---

*Last updated: 2025-12-28*
