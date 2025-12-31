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
- British spelling in docs (colour, behaviour, organisation)
- Conventional commits: `feat:`, `fix:`, `docs:`, `chore:`

## Off Limits

**`CODE_OF_CONDUCT.md`** — Do not modify. Owned by repository owner.

## PR Discipline

- One feature per PR
- Keep PRs small and focused (~5 files max)
- CI must pass before merging

## Project Structure

```
src/               # Rust source code
config/            # Example configuration files
TODO.md            # Development roadmap and progress
```

## Current Status

See `TODO.md` for the development roadmap and current phase.
