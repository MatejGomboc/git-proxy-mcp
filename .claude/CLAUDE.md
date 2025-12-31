# git-proxy-mcp

Secure Git proxy MCP server in Rust. Credentials stay on user's PC, never transmitted to AI.

## Commands

```bash
cargo build                    # Build
cargo test                     # Test
cargo clippy -- -D warnings    # Lint
cargo fmt                      # Format
```

## Key Rules

- **Security first**: Credentials NEVER in logs, errors, MCP responses, or debug output
- Use `secrecy::SecretString` for all credential handling
- British spelling in docs 🇬🇧 (colour, behaviour, organisation)
- Conventional commits: `feat:`, `fix:`, `docs:`, `chore:`

## 🚫 OFF LIMITS — All AI Specialists

**`CODE_OF_CONDUCT.md`** — Owned exclusively by the repository owner and their leadership. No AI specialist may modify this file under any circumstances. If changes are needed, flag it in JOURNAL.md for the owner.

## Sprint Discipline — IMPORTANT!

**ONE feature per PR. No exceptions.**

| ✅ Good | ❌ Bad |
|---------|--------|
| "Add config file parsing" | "Add config, auth, and git clone" |
| "Implement Windows Credential Manager" | "Implement all platform credential stores" |
| "Add error types for auth module" | "Complete Phase 1" |

### Why This Matters

Anthropic's research found that agents fail when they try to "one-shot" everything. Massive PRs:
- Are impossible to review
- Break in subtle ways
- Block other specialists
- Create merge conflicts

### Sprint Rules

1. **Scope small** — If your PR touches more than ~5 files, it's probably too big
2. **One concern** — Each PR should do ONE thing well
3. **Shippable** — PR must leave the repo in a working state (CI passes!)
4. **Stop early** — If scope is growing, stop, create PR, handoff to next specialist
5. **Iterate** — Multiple small PRs > one massive PR

### When To Stop and Create PR

- You've implemented one logical feature
- You've been working for a while and have meaningful progress
- The change is getting complex
- You need input from another specialist
- CI is passing and code works

**Create the PR, update JOURNAL.md, invoke next specialist!**

## Project Structure

```
src/               # Rust source code
config/            # Example configuration files  
.claude/           # AI assistant context
  commands/        # Specialist agent prompts
  JOURNAL.md       # Development log & handoff notes
  features.json    # Feature tracking (pass/fail)
TODO.md            # Development battle plan
```

## Virtual Software Team

This project uses specialist Claude agents invoked via `/project:` commands.

### Round-Robin Chain

```
🔒 Security → ⚙️ Core → 🪟 Windows → 🍎 macOS → 🐧 Linux → 🧪 QA → 📝 Docs → 🚀 DevOps
     ↑                                                                              │
     └──────────────────────────────────────────────────────────────────────────────┘
```

| Command | Specialist | Next in Chain |
|---------|------------|---------------|
| `/project:security` | 🔒 Security Lead | → `/project:core` |
| `/project:core` | ⚙️ Core Developer | → `/project:windows` |
| `/project:windows` | 🪟 Windows | → `/project:macos` |
| `/project:macos` | 🍎 macOS | → `/project:linux` |
| `/project:linux` | 🐧 Linux | → `/project:qa` |
| `/project:qa` | 🧪 QA | → `/project:docs` |
| `/project:docs` | 📝 Docs Pedant | → `/project:devops` |
| `/project:devops` | 🚀 DevOps | → `/project:security` |

### Handoff Protocol

Each specialist, when finishing:

1. **Push code** with conventional commit message
2. **Ask user: "Is CI passing?"** ← CRITICAL! Wait for confirmation
3. **Fix any CI failures** before proceeding
4. **Create PR** with small, focused changes (see Sprint Discipline!)
5. **Update JOURNAL.md** — what was done, what's next
6. **Update features.json** — mark features passing if verified

Next specialist:

1. **Read JOURNAL.md** — get up to speed (check who last worked!)
2. **Check `git log --oneline -10`** — see recent changes
3. **Read features.json** — know what's done/pending

### If Blocked or Nothing To Do

If a specialist encounters issues or has no work for their domain:

1. Update `JOURNAL.md` explaining the situation
2. **Invoke next specialist** in the chain

This keeps the round-robin moving!

## Current Phase

See @.claude/JOURNAL.md for current status and @TODO.md for full roadmap.
