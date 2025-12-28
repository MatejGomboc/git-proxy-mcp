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

This project uses specialist Claude agents invoked via `/project:` commands:

| Command | Specialist | Focus |
|---------|------------|-------|
| `/project:security` | 🔒 Security Lead | Credentials, auth, security review |
| `/project:core` | ⚙️ Core Developer | MCP protocol, git2, architecture |
| `/project:windows` | 🪟 Windows | Credential Manager, Windows paths |
| `/project:macos` | 🍎 macOS | Keychain, Apple Silicon |
| `/project:linux` | 🐧 Linux | Secret Service, XDG paths |
| `/project:devops` | 🚀 DevOps | CI/CD, releases, caching |
| `/project:docs` | 📝 Docs Pedant | Repo cleanliness, British spelling |
| `/project:qa` | 🧪 QA | Testing, edge cases |

### Round-Robin Workflow

Specialists take turns, each with fresh context:

```
Security → Core → Platform Devs → Security Review → QA → Docs
    └──────────────────────────────────────────────────────┘
                         (next feature)
```

### Handoff Protocol

Each specialist, when finishing:

1. **Commit** with conventional commit message
2. **Update JOURNAL.md** — what was done, what's next
3. **Update features.json** — mark features passing if verified

Next specialist:

1. **Read JOURNAL.md** — get up to speed
2. **Check `git log --oneline -10`** — see recent changes
3. **Read features.json** — know what's done/pending

## Current Phase

See @.claude/JOURNAL.md for current status and @TODO.md for full roadmap.
