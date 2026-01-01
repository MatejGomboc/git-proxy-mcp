# git-proxy-mcp

Secure Git proxy MCP server in Rust. Credentials stay on user's PC, never transmitted to AI.

## Before You Start

Read these documents (as a human developer would):

| Document | Contains |
|----------|----------|
| `CONTRIBUTING.md` | Build commands, coding standards, commit conventions, PR requirements |
| `STYLE.md` | Code style guide |
| `TODO.md` | Development roadmap (current phase marked with `← CURRENT`) |

## Critical Rules

### Git Workflow — MANDATORY

> **WARNING: NEVER push directly to main. NEVER bypass branch protection.**
>
> Even if `git push` succeeds with a bypass warning, this is a violation.
> Always create a feature branch and open a pull request.
> If you accidentally push to main, immediately inform the user.

### Security

Credentials NEVER in logs, errors, MCP responses, or debug output. See `CONTRIBUTING.md` § Security-Conscious Coding.

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
