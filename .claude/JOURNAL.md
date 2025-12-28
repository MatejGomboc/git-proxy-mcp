# Development Journal

Handoff document for Claude specialist agents. Read this first to understand current status.

---

## Current Status

**Phase:** 0.6 Complete → Ready for Phase 1

**Last Specialist:** 🚀 DevOps (CI optimisation)

**Completed:**
- ✅ Phase 0: Project setup (Cargo.toml, CI, VS Code config)
- ✅ Phase 0.5: Open source best practices
- ✅ Phase 0.6: CI/CD optimisation (caching, job consolidation)

**Next:** Phase 1 — Core Infrastructure
1. `src/config/` — Config file loading & validation
2. `src/auth/` — Credential management with `secrecy` crate
3. `src/error.rs` — Custom error types

**Suggested First Specialist:** 🔒 Security (design credential architecture)

---

## Virtual Team

| Specialist | Command | Status |
|------------|---------|--------|
| 🔒 Security Lead | `/project:security` | Ready |
| ⚙️ Core Developer | `/project:core` | Ready |
| 🪟 Windows | `/project:windows` | Ready |
| 🍎 macOS | `/project:macos` | Ready |
| 🐧 Linux | `/project:linux` | Ready |
| 🚀 DevOps | `/project:devops` | Ready |
| 📝 Docs Pedant | `/project:docs` | Ready |
| 🧪 QA | `/project:qa` | Ready |

---

## Handoff Template

When ending your session, add an entry like this:

```markdown
### YYYY-MM-DD — [Specialist Emoji] Brief Title

**Specialist:** [Your role]

**What I did:**
- Thing 1
- Thing 2

**Decisions made:**
- Decision and rationale

**For next specialist:**
- What needs to happen next
- Any blockers or concerns

**Features updated:** (if any)
- `feature_name`: now passing ✅
```

---

## Session Log

### 2025-12-28 — 🚀 Virtual Team Setup

**Specialist:** DevOps (setting up team infrastructure)

**What I did:**
- Created specialist command files in `.claude/commands/`
- Updated CLAUDE.md with team protocol
- Set up round-robin workflow documentation

**For next specialist:**
- Phase 1 ready to begin
- Suggest starting with 🔒 Security to design credential architecture
- Then ⚙️ Core to implement config loading
- Then platform specialists for credential stores

---

### 2025-12-28 — Remove Devcontainer

**Decision:** Removed `.devcontainer/` folder.

**Rationale:**
- Contributors are Rust developers with rustup installed
- CI tests all 3 platforms anyway
- Native development provides better debugging
- Devcontainer only supports Linux
- Less maintenance overhead

---

### 2025-12-28 — CI/CD Optimisation (Phase 0.6)

**Problem:** CI was taking ~8 minutes per PR.

**Solution:** Applied StringWiggler caching pattern:
- Added `Swatinem/rust-cache@v2` for cargo registry/target caching
- PRs use read-only cache, main branch saves cache
- Combined 5 jobs into 2 (quick-checks + build matrix)
- Eliminated redundant compilation across jobs

**Result:** ~2 minutes on cache hit (75% faster!)

**CI Architecture:**
```
quick-checks (ubuntu)     build (matrix: ubuntu, macos, windows)
├── fmt                   ├── clippy
└── docs                  ├── build (debug + release)
                          └── test
```

---

### 2025-12-28 — Claude Code Setup & Phase 0.5 Completion

**What happened:**
- Reorganised `.claude/` folder to follow Claude Code best practices
- Renamed `INSTRUCTIONS.md` → `CLAUDE.md` (Claude Code's expected filename)
- Owner completed remaining setup:
  - Created CODE_OF_CONDUCT.md
  - Enabled Secret Scanning & Push Protection
  - Set up branch protection for `main`
  - Set up tag protection
- Phase 0 + 0.5 now fully complete!

---

### 2025-12-28 — Phase 0.5 Documentation

**What happened:**
- Created CONTRIBUTING.md, CHANGELOG.md
- Created issue templates (bug report, feature request)
- Created PR template with security checklist
- Moved AI docs to `.claude/` folder

---

### 2025-12-28 — Phase 0 Setup

**What happened:**
- Created Cargo.toml, main.rs skeleton
- Set up CI workflow for Rust (fmt, clippy, build, test)
- Created example config file
- Fixed clippy lints

---

## Architecture

```
Credentials: config.json → Auth Module → GitHub API
                              ↓
                         (internally)
                              ↓
Git Data: GitHub API → git2-rs → MCP Response → AI VM

Credentials NEVER appear in MCP responses.
```

## Key Crates

| Crate | Purpose |
|-------|---------|
| `git2` | Git operations (libgit2) |
| `tokio` | Async runtime |
| `serde` | JSON parsing |
| `clap` | CLI args |
| `tracing` | Logging (stderr only) |
| `secrecy` | Credential handling |

---

*Last updated: 2025-12-28*
