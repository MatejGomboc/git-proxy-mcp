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
src/           # Rust source code
config/        # Example configuration files  
.claude/       # AI assistant context
  JOURNAL.md   # Development history & session handoff
  features.json # Feature tracking (pass/fail)
TODO.md        # Development battle plan
```

## Current Phase

See @.claude/JOURNAL.md for current status and @TODO.md for full roadmap.

## Session Workflow

1. Read `.claude/JOURNAL.md` for current status
2. Check `git log --oneline -10` for recent changes
3. Work on ONE feature at a time
4. Update JOURNAL.md at session end
5. Ensure CI passes before finishing
