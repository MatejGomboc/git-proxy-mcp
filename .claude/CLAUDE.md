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

### Git Workflow — MANDATORY

> **WARNING: NEVER push directly to main. NEVER bypass branch protection.**
>
> Even if `git push` succeeds with a bypass warning, this is a violation.
> Always create a feature branch and open a pull request.
> If you accidentally push to main, immediately inform the user.

### Security

Credentials NEVER in logs, errors, MCP responses, or debug output. See `CONTRIBUTING.md` § Security-Conscious Coding.

**Before committing**, always clean up stale branches:

```bash
git fetch --prune origin
git branch -vv | grep ': gone]' | awk '{print $1}' | xargs -r git branch -d
```

This removes local branches whose remote tracking branch has been deleted.

### Task Management

**Remove completed items from `TODO.md`** after finishing a task. Keep the roadmap current by deleting done items.

## Off Limits

**`CODE_OF_CONDUCT.md`** — Do not modify. Owned by repository owner.

## Project Structure

```
src/               # Rust source code
config/            # Example configuration files
```
