# Development Journal

Handoff document for Claude specialist agents. Read this first to understand current status.

---

## Current Status

**Phase:** 1 — Core Infrastructure (in progress)

**Last Specialist:** 🔒 Security Lead (credential types)

**Completed:**
- ✅ Phase 0: Project setup (Cargo.toml, CI, VS Code config)
- ✅ Phase 0.5: Open source best practices
- ✅ Phase 0.6: CI/CD optimisation (caching, job consolidation)
- ✅ `src/auth/credentials.rs` — Secure credential types with secrecy crate
- ✅ `src/error.rs` — Error types (security-aware)

**Next:**
1. `src/config/` — Config file loading & validation (parse into credential types)
2. `src/auth/matcher.rs` — URL pattern matching for credentials
3. Wire up config loading in main.rs

**Suggested Next Specialist:** ⚙️ Core Developer (`/project:core`)

---

## Virtual Team

| Specialist | Command | Status |
|------------|---------|--------|
| 🔒 Security Lead | `/project:security` | ✅ Done (credential types) |
| ⚙️ Core Developer | `/project:core` | 👈 Next up |
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

### 2025-12-28 — 🔒 Secure Credential Types

**Specialist:** Security Lead

**What I did:**
- Created `src/auth/mod.rs` with security architecture documentation
- Created `src/auth/credentials.rs` with:
  - `PatCredential` — PAT with `SecretString`
  - `SshKeyCredential` — SSH key path + optional passphrase
  - `SshAgentCredential` — SSH agent auth
  - `Credential` — URL pattern wrapper
  - `AuthMethod` — Enum unifying all types
- Created `src/error.rs` with `AuthError` and `ConfigError`
- Updated `src/main.rs` to declare new modules
- Added unit tests verifying `Debug` doesn't leak secrets

**Decisions made:**
- All sensitive data MUST use `secrecy::SecretString`
- Custom `Debug` impls show `[REDACTED]` for secrets
- No `Display` impl for credential types (prevents accidental printing)
- Explicit `expose_*()` methods required to access secrets
- Error messages intentionally omit credential values
- Helper `deserialize_secret()` for future config parsing

**Security properties verified:**
- ✅ `SecretString` zeroises on drop
- ✅ `Debug` output tested to not contain secrets
- ✅ Error messages tested to not contain credential patterns

**For next specialist (⚙️ Core):**
- Implement `src/config/mod.rs` and `src/config/settings.rs`
- Parse `config.json` into the credential types I created
- Use the `deserialize_secret()` helper for token fields
- Create URL matcher in `src/auth/matcher.rs` using `glob` crate
- The `Credential` struct is ready to hold parsed config entries

**PR:** https://github.com/MatejGomboc/git-proxy-mcp/pull/7

---

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
