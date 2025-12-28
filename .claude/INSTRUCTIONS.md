# Claude Instructions for git-proxy-mcp

This folder contains guidance and context for Claude (and other AI assistants) working on this project.

## Quick Start

**Read these files in order:**

1. **`INSTRUCTIONS.md`** (this file) — Overview and guidelines
2. **`JOURNAL.md`** — Development history and current status
3. **`features.json`** — Feature tracking with pass/fail status
4. **`../TODO.md`** — Full development battle plan (in repo root)

## Getting Up to Speed

At the start of EVERY session, run:

```bash
# 1. Read the journal to understand current status
cat .claude/JOURNAL.md

# 2. Read the battle plan
cat TODO.md

# 3. See recent commits
git log --oneline -10

# 4. Check feature completion status
cat .claude/features.json

# 5. Verify project compiles
cargo build

# 6. Verify tests pass
cargo test

# 7. Check for warnings
cargo clippy
```

**Then:** Find the next incomplete feature in `features.json` and work on ONE feature at a time.

## End of Session Checklist

Before ending your session:

1. ✅ Commit changes with descriptive conventional commit messages
2. ✅ Update `.claude/JOURNAL.md` with what you did
3. ✅ Update `.claude/features.json` if you completed any features
4. ✅ Ensure CI passes (no broken builds!)
5. ✅ Leave codebase in a clean state

## Project Overview

**git-proxy-mcp** is a secure Git proxy MCP server that:
- Keeps credentials on the user's PC (never transmitted to AI)
- Allows AI assistants to clone/pull/push to private repos
- Works with any MCP-compatible client (Claude Desktop, Cursor, etc.)
- Written in Rust for security and performance

## Critical Security Rules

⚠️ **CREDENTIALS MUST NEVER:**
- Appear in MCP responses
- Appear in log messages
- Appear in error messages
- Appear in debug output
- Be committed to the repository

Use the `secrecy` crate for all credential handling.

## Style Guidelines

### British Spelling 🇬🇧

Use British spelling in all documentation and user-facing text:

| ❌ American | ✅ British |
|-------------|------------|
| color | colour |
| behavior | behaviour |
| organization | organisation |
| center | centre |
| license (noun) | licence |
| analyze | analyse |
| initialize | initialise |

### Commit Messages

Use [Conventional Commits](https://www.conventionalcommits.org/):

```
feat: add clone progress streaming
fix: prevent credential leak in error messages
docs: update README with installation instructions
chore: update dependencies
```

## Key Files

| File | Purpose |
|------|---------|
| `.claude/JOURNAL.md` | Development history, session logs, handoff notes |
| `.claude/features.json` | Feature tracking (only change `passes` field) |
| `TODO.md` | Full development battle plan |
| `Cargo.toml` | Rust dependencies |
| `src/main.rs` | Entry point |
| `config/example-config.json` | Example configuration |

## Links

- **Repo:** https://github.com/MatejGomboc/git-proxy-mcp
- **MCP Spec:** https://modelcontextprotocol.io
- **git2-rs:** https://docs.rs/git2

---

*This file is for AI assistants. Humans should see README.md and CONTRIBUTING.md.*
