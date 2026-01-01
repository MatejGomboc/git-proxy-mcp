# git-proxy-mcp

Secure Git proxy MCP server in Rust. Uses your existing Git credential configuration — no credentials stored.

## Quick Reference

| What | Where |
|------|-------|
| Build commands | `CONTRIBUTING.md` § Development Setup |
| Coding standards | `CONTRIBUTING.md` § Coding Standards |
| Style guide | `STYLE.md` |
| Commit conventions | `CONTRIBUTING.md` § Commit Messages |
| PR requirements | `CONTRIBUTING.md` § Pull Requests |
| Development roadmap | `TODO.md` |

## Architecture

The MCP server is a **credential-free proxy** that spawns git commands:

```
User's Git Config (credential helpers, ssh-agent)
          ↓ git uses these automatically
git-proxy-mcp (validates command, spawns git, sanitises output)
          ↓ stdio
MCP Client (Claude Desktop, etc.)
          ↓ TLS
AI (Claude, GPT, etc.)
```

**Key point:** No credentials in config.json — just security settings.

## Critical Rules

### Git Workflow — MANDATORY

> **WARNING: NEVER push directly to main. NEVER bypass branch protection.**
>
> Even if `git push` succeeds with a bypass warning, this is a violation.
> Always create a feature branch and open a pull request.
> If you accidentally push to main, immediately inform the user.

### Security

- The MCP server does NOT store credentials
- All git output is sanitised for credential leaks
- See `CONTRIBUTING.md` § Security-Conscious Coding

### Before Committing

Clean up stale branches:

```bash
git fetch --prune origin
git branch -vv | grep ': gone]' | awk '{print $1}' | xargs -r git branch -d
```

### Task Management

**Remove completed items from `TODO.md`** after finishing a task. Keep the roadmap current.

## Off Limits

**`CODE_OF_CONDUCT.md`** — Do not modify. Owned by repository owner.

## Project Structure

```
src/
├── config/      # Configuration loading (security settings only)
├── error.rs     # Error types
├── git/         # Git command parsing, execution, output sanitisation
├── mcp/         # MCP protocol, transport, server
└── security/    # Guards (branch, push, repo filter), audit, rate limiting
```
