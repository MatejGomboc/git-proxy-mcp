# git-proxy-mcp

Secure credential proxy enabling cloud AI assistants to work with private Git repositories. Uses git2 (libgit2) for streaming operations. No credentials leave the user's machine; no files persist on the user's machine.

## Quick Reference

| What | Where |
|------|-------|
| Build commands | `CONTRIBUTING.md` § Development Setup |
| Coding standards | `CONTRIBUTING.md` § Coding Standards |
| Style guide | `STYLE.md` |
| Commit conventions | `CONTRIBUTING.md` § Commit Messages |
| PR requirements | `CONTRIBUTING.md` § Pull Requests |
| Development roadmap | `TODO.md` |

## Architecture (v2)

The MCP server is a **pure credential proxy** — it streams Git data between providers and AI VMs:

```
GitHub/GitLab                 User's PC                   AI's VM
      │                           │                          │
      │◄── git2 auth + fetch ────►│◄── MCP stream ──────────►│
      │                           │                          │
      │                     Credentials                 Full repo
      │                     stay here                  lives here
```

**Key principles:**
- Credentials NEVER leave user's machine
- Files NEVER persist on user's machine (stream-through only)
- AI maintains complete local git repo in its VM
- Uses git2 library (not git CLI subprocess)

## Target Users

- ✅ Claude.ai (with computer use)
- ✅ ChatGPT with Code Interpreter
- ✅ Gemini with code execution
- ✅ Any cloud AI with sandboxed compute
- ❌ Claude Code (already has local access)
- ❌ Cursor (already has local access)

## MCP Tools

| Tool | Purpose |
|------|--------|
| `repo/clone` | Stream repository to AI's VM |
| `repo/push` | Push commits from AI's VM to remote |
| `repo/pull` | Sync new changes to AI's VM |

## Critical Rules

### Git Workflow — MANDATORY

> **WARNING: NEVER push directly to main. NEVER bypass branch protection.**
>
> Always create a feature branch and open a pull request.
> If you accidentally push to main, immediately inform the user.

### Security

- Credentials handled via git2 callbacks to system credential helpers
- No credentials stored in MCP server state
- All streaming data is repo content only, never auth tokens
- Audit logging for all operations

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
├── config/      # Configuration loading
├── error.rs     # Error types
├── git2/        # git2 library integration (NEW in v2)
│   ├── auth.rs      # Credential callbacks
│   ├── clone.rs     # Streaming clone
│   ├── push.rs      # Patch application and push
│   └── session.rs   # Repo session management
├── mcp/         # MCP protocol, transport, server
│   └── tools/       # repo/clone, repo/push, repo/pull
├── security/    # Guards, audit, rate limiting
└── streaming/   # Tar/archive handling (NEW in v2)
```

## Development Notes

### git2 Credential Callbacks

```rust
let mut callbacks = git2::RemoteCallbacks::new();
callbacks.credentials(|_url, username, allowed| {
    // Use system credential helper — NEVER store credentials
    git2::Cred::credential_helper(&git_config, url, username)
});
```

### Streaming Pattern

```rust
// Clone: stream directly from remote to MCP response
// Never write to disk on MCP server side
repo.find_tree(commit.tree_id())?
    .walk(TreeWalkMode::PreOrder, |_, entry| {
        // Stream each blob directly
    });
```

### Testing with git2

```bash
# Integration tests need a test repo
cargo test --features integration

# Unit tests work standalone  
cargo test
```
