# Development Journal

Handoff document for Claude instances. Read this first to understand current status.

---

## Current Status

**Phase:** 0.6 Complete → Ready for Phase 1

**Completed:**
- ✅ Phase 0: Project setup (devcontainer, Cargo.toml, CI)
- ✅ Phase 0.5: Open source best practices
- ✅ Phase 0.6: CI/CD optimisation (caching, job consolidation)

**Next:** Phase 1 — Core Infrastructure
1. `src/config/` — Config file loading & validation
2. `src/auth/` — Credential management with `secrecy` crate
3. `src/error.rs` — Custom error types

---

## Session Log

### 2025-12-28 — CI/CD Optimisation (Phase 0.6)

**Problem:** CI was taking ~8 minutes per PR.

**Solution:** Applied StringWiggler caching pattern:
- Added `Swatinem/rust-cache@v2` for cargo registry/target caching
- PRs use read-only cache, main branch saves cache
- Combined 5 jobs into 2 (quick-checks + build matrix)
- Eliminated redundant compilation across jobs

**Expected improvement:** ~50-70% faster on cache hits (~2-3 min)

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
- Created devcontainer, Cargo.toml, main.rs skeleton
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

## Tips

1. Work on ONE feature at a time
2. Security is paramount — credentials must NEVER leak
3. British spelling in docs 🇬🇧
4. Conventional commits (`feat:`, `fix:`, `docs:`)
5. Update this journal at session end

---

*Last updated: 2025-12-28*
