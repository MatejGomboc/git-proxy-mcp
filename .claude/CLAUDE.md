# git-proxy-mcp

Secure Git proxy MCP server in Rust. Credentials stay on user's PC, never transmitted to AI.

## Quick Reference

| What | Where |
|------|-------|
| Build commands | `CONTRIBUTING.md` § Development Setup |
| Coding standards | `CONTRIBUTING.md` § Coding Standards |
| Style guide | `STYLE.md` |
| Commit conventions | `CONTRIBUTING.md` § Commit Messages |
| PR requirements | `CONTRIBUTING.md` § Pull Requests |
| Development roadmap | `TODO.md` |

## Critical Rules

### Security

Credentials NEVER in logs, errors, MCP responses, or debug output. See `CONTRIBUTING.md` § Security-Conscious Coding.

### Git Workflow

**NEVER push directly to main.** Always create a feature branch and open a pull request. Do not bypass repository branch protection rules under any circumstances.

## Off Limits

**`CODE_OF_CONDUCT.md`** — Do not modify. Owned by repository owner.

## Project Structure

```
src/               # Rust source code
config/            # Example configuration files
```
