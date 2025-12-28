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

1. **Commit** with conventional commit message
2. **Update JOURNAL.md** — what was done, what's next
3. **Update features.json** — mark features passing if verified

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
