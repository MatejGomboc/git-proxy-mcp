# Development Journal

This file tracks the development progress of `git-proxy-mcp`. It serves as a handoff document between Claude instances so each new instance knows what was done and what to do next.

---

## Getting Up to Speed (for Claude Instances)

**Run these steps at the start of EVERY session:**

```bash
# 1. Verify you're in the project directory
pwd

# 2. Read this journal to understand current status
cat JOURNAL.md

# 3. Read the full battle plan
cat TODO.md

# 4. See recent commits
git log --oneline -10

# 5. Check feature completion status
cat features.json

# 6. Verify project compiles (once Cargo.toml exists)
cargo build

# 7. Verify tests pass (once tests exist)
cargo test

# 8. Check for warnings
cargo clippy
```

**Then:** Find the next incomplete feature in `features.json` and work on ONE feature at a time.

**At the end of your session:**
1. Commit your changes with a descriptive message
2. Update this JOURNAL.md with what you did
3. Update `features.json` if you completed any features

---

## How to Use This Journal

**For Claude instances:** Follow "Getting Up to Speed" above, then read "Current Status".

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

### 2025-12-28 — Phase 0.5 Documentation Completion (Claude + MatejGomboc)

**What happened:**
- Continued from previous Claude instance that was cut off
- Created all remaining open source documentation files
- Skipped CODE_OF_CONDUCT.md (requires owner's personal contact info for enforcement)

**Files created:**
- `CONTRIBUTING.md` — Comprehensive contributor guidelines including:
  - Bug reporting process with security reminders
  - Feature request process
  - Pull request workflow and checklist
  - Coding standards (rustfmt, clippy, British spelling)
  - Commit message format (conventional commits)
  - Testing requirements
  - Security-conscious coding guidelines
- `CHANGELOG.md` — Keep a Changelog format with:
  - Initial project setup items documented
  - Versioning policy explained
- `.github/ISSUE_TEMPLATE/bug_report.md` — Bug report template with:
  - Security reminder to redact credentials
  - Environment information fields
  - Link to security advisories for vulnerabilities
- `.github/ISSUE_TEMPLATE/feature_request.md` — Feature request template with:
  - Security considerations section
  - Checklist for contributors
- `.github/ISSUE_TEMPLATE/config.yml` — Issue template configuration:
  - Disabled blank issues (require template)
  - Added link to security advisories
  - Added link to discussions
- `.github/PULL_REQUEST_TEMPLATE.md` — PR template with:
  - Security checklist for credential handling
  - Testing and code quality checklists
  - Reminder to update CHANGELOG

**Files modified:**
- `TODO.md` — Updated Phase 0.5 checklist to reflect completed items
- `JOURNAL.md` — This file, updated with session log

**Commits made:**
1. `docs: Add CONTRIBUTING.md with contributor guidelines`
2. `docs: Add CHANGELOG.md (Keep a Changelog format)`
3. `docs: Add bug report issue template`
4. `docs: Add feature request issue template`
5. `docs: Add issue template config`
6. `docs: Add pull request template`
7. `docs: Update TODO.md with Phase 0.5 progress`
8. `docs: Update JOURNAL.md with session log`

**Note on CODE_OF_CONDUCT.md:**
Skipped because it requires personal contact information for enforcement. The Contributor Covenant template (https://www.contributor-covenant.org/version/2/1/code_of_conduct/) needs an email address where people can report violations. Owner should create this file with their own contact info.

---

### 2025-12-28 — SECURITY.md Created (Claude + MatejGomboc)

**What happened:**
- Created comprehensive SECURITY.md with:
  - Vulnerability disclosure policy (GitHub Security Advisories)
  - Security update policy
  - Supported versions (development phase)
  - Response timeline expectations
  - Security best practices for users
  - Security design principles

**Files created:**
- `SECURITY.md` — Comprehensive security policy

**Commits made:**
1. `docs: Update SECURITY.md with comprehensive security policy`

---

### 2025-12-28 — Open Source Best Practices Review (Claude + MatejGomboc)

**What happened:**
- Reviewed https://opensource.guide/ for open source project best practices
- Researched security best practices, code of conduct guidelines, documentation standards
- Identified gaps in current project setup vs recommended practices
- Added new "Phase 0.5: Open Source Best Practices" section to TODO.md

**Key findings from opensource.guide:**
- SECURITY.md is **critical** for a credential-handling project
- CODE_OF_CONDUCT.md establishes community expectations (Contributor Covenant recommended)
- CONTRIBUTING.md helps onboard new contributors
- Issue and PR templates standardise contributions
- Secret Scanning and Push Protection should be enabled

**Files modified:**
- `TODO.md` — Added Phase 0.5 with detailed open source best practices tasks

**Commits made:**
1. `docs: Add Phase 0.5 Open Source Best Practices to TODO.md`

**References added:**
- https://opensource.guide/
- https://www.contributor-covenant.org/
- https://keepachangelog.com/
- https://www.conventionalcommits.org/

---

### 2025-12-28 — Repository Configuration & CI Fixes (Claude + MatejGomboc)

**What happened:**
- Configured GitHub repository settings:
  - Enabled GitHub Actions with restricted permissions
  - Added `dtolnay/rust-toolchain@stable` to allowed actions
  - Enabled CodeQL analysis for Rust and GitHub Actions
  - Enabled Copilot Autofix for CodeQL alerts
  - Configured code review limits
  - Set workflow permissions to read-only (principle of least privilege)
  - Configured fork PR approval requirements
- Fixed CI failures caused by clippy lints:
  - Changed `thiserror` from v2 to v1 (to match git2's dependency)
  - Added `multiple_crate_versions = "allow"` (transitive deps we can't control)
  - Added `#[allow(clippy::unnecessary_wraps)]` to main() with explanation

**Files modified:**
- `Cargo.toml` — Fixed thiserror version, added clippy lint allow
- `src/main.rs` — Added allow attribute and rustdoc for main()

**Commits made:**
1. `fix: Resolve clippy dependency version warnings`
2. `fix: Allow unnecessary_wraps lint on main()`

**Repository settings configured:**
| Setting | Value |
|---------|-------|
| Actions | Restricted (MatejGomboc + GitHub + dtolnay/rust-toolchain) |
| CodeQL | Enabled (Rust, GitHub Actions) |
| Copilot Autofix | On |
| Fork PR approval | All external contributors |
| Workflow permissions | Read-only |

---

### 2025-12-28 — Phase 0 Implementation (Claude + MatejGomboc)

**What happened:**
- Created `.devcontainer/devcontainer.json` and `Dockerfile`
- Created `Cargo.toml` with all planned dependencies
- Created `src/main.rs` with clap CLI skeleton
- Created `.gitignore`, `rust-toolchain.toml`, `rustfmt.toml`
- Updated CI workflow from C++ ARM to Rust (fmt, clippy, build, test, docs)
- Updated VS Code extensions and settings for Rust
- Created `config/example-config.json` with all auth types documented
- Fixed CI workflow action name (rust-action → rust-toolchain)

**Files created/modified:**
- `.devcontainer/devcontainer.json` — NEW
- `.devcontainer/Dockerfile` — NEW
- `Cargo.toml` — NEW
- `src/main.rs` — NEW
- `.gitignore` — NEW
- `rust-toolchain.toml` — NEW
- `rustfmt.toml` — NEW
- `.github/workflows/ci_pr_validation.yml` — REPLACED (was C++)
- `.vscode/extensions.json` — UPDATED (Rust extensions)
- `.vscode/settings.json` — UPDATED (rust-analyzer settings)
- `config/example-config.json` — NEW

**Commits made:**
1. `feat: Add devcontainer configuration`
2. `feat: Add Dockerfile for devcontainer`
3. `feat: Add Cargo.toml with all dependencies`
4. `feat: Add src/main.rs skeleton with CLI parsing`
5. `chore: Add .gitignore for Rust project`
6. `chore: Add rust-toolchain.toml for consistent Rust version`
7. `chore: Add rustfmt.toml for consistent code formatting`
8. `feat: Update CI workflow for Rust`
9. `feat: Update VS Code extensions for Rust`
10. `feat: Update VS Code settings for Rust`
11. `fix: Correct Rust toolchain action name`
12. `docs: Add example configuration file`

---

### 2025-12-28 — Initial Planning (Claude + MatejGomboc)

**What happened:**
- Discussed the concept of a Git proxy MCP server
- Designed the security architecture (credentials never leave user's PC)
- Chose Rust as the implementation language
- Chose GPL v3 as the licence
- Created comprehensive `TODO.md` with battle plan
- Resolved key design decisions:
  - No config hot-reload (security)
  - Yes concurrent operations
  - SSH keys: user manages, we reference
  - Large repos: chunked streaming
  - Git LFS: defer to v1.1
- Reviewed Anthropic's article on long-running agents
- Added `features.json` for structured feature tracking
- Added "Getting Up to Speed" routine for Claude instances

**Key files created:**
- `TODO.md` — Full development battle plan
- `JOURNAL.md` — This file, for session continuity
- `features.json` — Structured feature tracking with pass/fail status

**Decisions made:**
| Decision | Choice |
|----------|--------|
| Language | Rust |
| Licence | GPL v3 |
| MCP Transport | stdio (v1.0) |
| Config reload | No (security) |
| Concurrency | Yes |
| Git LFS | v1.1 (detect & warn in v1.0) |
| Spelling | British 🇬🇧 |
| Feature tracking | `features.json` (not Markdown) |

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

## File Structure (Current)

```
git-proxy-mcp/
├── .devcontainer/
│   ├── devcontainer.json  ✅
│   └── Dockerfile         ✅
├── .github/
│   ├── CODEOWNERS         ✅
│   ├── dependabot.yml     ✅
│   ├── PULL_REQUEST_TEMPLATE.md  ✅ NEW
│   ├── ISSUE_TEMPLATE/
│   │   ├── bug_report.md      ✅ NEW
│   │   ├── feature_request.md ✅ NEW
│   │   └── config.yml         ✅ NEW
│   └── workflows/
│       └── ci_pr_validation.yml  ✅
├── .vscode/
│   ├── extensions.json    ✅
│   └── settings.json      ✅
├── config/
│   └── example-config.json  ✅
├── src/
│   └── main.rs            ✅
├── Cargo.toml             ✅
├── rust-toolchain.toml    ✅
├── rustfmt.toml           ✅
├── .gitignore             ✅
├── .editorconfig          ✅
├── CHANGELOG.md           ✅ NEW
├── CONTRIBUTING.md        ✅ NEW
├── features.json          ✅
├── JOURNAL.md             ✅
├── LICENCE                ✅
├── README.md              ✅ (basic, needs expansion)
├── SECURITY.md            ✅
└── TODO.md                ✅
```

**Files still to add:**
```
├── CODE_OF_CONDUCT.md     ⏳ Owner to create (requires personal contact)
```

---

## Style Guidelines

### British Spelling 🇬🇧

Use British spelling throughout documentation and user-facing text. It's posh!

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

**Note:** Code identifiers (variable names, function names) can use American spelling if it matches library conventions (e.g., Rust stdlib uses American spelling).

---

## Tips for Future Claude Instances

1. **Follow "Getting Up to Speed"** at the start of every session
2. **Work on ONE feature at a time** — don't try to do too much
3. **Update this journal** after each session with what you did and what's next
4. **Update `features.json`** when you complete features — only change `passes` field
5. **Security is paramount** — credentials must NEVER leak to logs, errors, or MCP responses
6. **Test on all platforms** — Windows, macOS, Linux
7. **Keep commits atomic** — one logical change per commit
8. **Use conventional commits** — `feat:`, `fix:`, `docs:`, `chore:`, etc.
9. **Use British spelling** in docs and user-facing text — colour, behaviour, organisation, centre, licence, etc. 🇬🇧
10. **Leave the codebase in a clean state** — no half-finished features, no broken builds

---

## Links

- **Repo:** https://github.com/MatejGomboc/git-proxy-mcp
- **MCP Spec:** https://modelcontextprotocol.io
- **git2-rs:** https://docs.rs/git2
- **Rust MCP examples:** https://github.com/modelcontextprotocol/servers
- **Long-running agents guide:** https://www.anthropic.com/engineering/effective-harnesses-for-long-running-agents
- **Open Source Guides:** https://opensource.guide/
- **Contributor Covenant:** https://www.contributor-covenant.org/
- **Keep a Changelog:** https://keepachangelog.com/
- **Conventional Commits:** https://www.conventionalcommits.org/

---

*Last updated: 2025-12-28*
