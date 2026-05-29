# git-proxy-mcp

[![CI](https://github.com/MatejGomboc/git-proxy-mcp/actions/workflows/ci_main.yml/badge.svg)](https://github.com/MatejGomboc/git-proxy-mcp/actions/workflows/ci_main.yml)
[![codecov](https://codecov.io/gh/MatejGomboc/git-proxy-mcp/branch/main/graph/badge.svg)](https://codecov.io/gh/MatejGomboc/git-proxy-mcp)

**Your Git credentials stay on your machine. Your repo lives in the AI's workspace.**

A secure MCP server that lets cloud-based AI assistants (Claude.ai, ChatGPT, Gemini, etc.)
work with private Git repositories using your existing Git credentials — without those
credentials ever leaving your machine.

---

## The Problem

Cloud-based AI coding assistants face a fundamental dilemma:

| Approach | Problem |
|----------|--------|
| **GitHub MCP Server** | File-by-file API calls. 50 files = 50 calls. Can't run tests. Painfully slow. |
| **Give AI your credentials** | Security nightmare. Your PATs/SSH keys in someone else's cloud. |
| **Only use public repos** | Most real work is on private repositories. |

**The result:** AI assistants that can write code but can't actually work on your projects like a real developer would.

## The Solution

git-proxy-mcp acts as an **authenticated streaming proxy** between Git providers and AI workspaces:

```text
┌─────────────────┐      ┌─────────────────┐      ┌─────────────────┐
│  Git Providers  │      │    YOUR PC      │      │    AI's VM      │
│                 │      │                 │      │                 │
│  GitHub         │◄────►│  git-proxy-mcp  │◄────►│  Claude.ai      │
│  GitLab         │      │                 │      │                 │
│  Bitbucket      │      │  (credentials   │      │  /home/claude/  │
│  Azure DevOps   │      │   stay here)    │      │    repo/        │
│  Self-hosted    │      │                 │      │  (files live    │
│                 │      │                 │      │   here)         │
└─────────────────┘      └─────────────────┘      └─────────────────┘
```

**Key insight:** The AI has its own VM with full Linux capabilities. It just can't authenticate to your private repos. We solve *only* that problem.

### How It Works

1. **Clone:** AI requests a repo → MCP server authenticates → streams files directly to AI's VM
2. **Work:** AI has a complete local git repo. Branch, edit, test, commit — all native.
3. **Push:** AI sends commits → MCP server authenticates → pushes to remote

**Credentials never leave your machine. Files never touch your machine.**

---

## Who Is This For?

| Environment | Local Git? | Needs This? | Why |
|-------------|------------|-------------|-----|
| **Claude.ai** | ❌ Cloud VM | ✅ **YES** | Has compute, lacks credentials |
| **ChatGPT + Code Interpreter** | ❌ Sandboxed | ✅ **YES** | Same situation |
| **Gemini + code execution** | ❌ Sandboxed | ✅ **YES** | Same situation |
| **Any cloud AI with VM** | ❌ | ✅ **YES** | Universal solution |
| Claude Code | ✅ Local | ❌ No | Already has direct access |
| Cursor | ✅ Local | ❌ No | Runs on your machine |
| GitHub Copilot | ✅ Local | ❌ No | IDE extension |

---

## Comparison: GitHub MCP vs git-proxy-mcp

| Operation | GitHub MCP Server | git-proxy-mcp |
|-----------|-------------------|---------------|
| Clone 100 files | 100 API calls, minutes | 1 streaming call, seconds |
| Run `cargo test` | ❌ Impossible | ✅ Native in AI's VM |
| Interactive rebase | ❌ Impossible | ✅ `git rebase -i` |
| Branch + edit + commit + push | 4+ API calls | Work locally, 1 push |
| View git log/diff | API calls | Instant local commands |
| Large repositories | Timeout hell | Shallow clone, sparse checkout |
| Rate limits | Hit constantly | Just auth, minimal API use |

---

## Architecture

### Security Model

```text
┌─────────────────────────────────────────────────────────────────┐
│  YOUR PC (credentials stay here, files don't)                   │
│                                                                 │
│  ┌──────────────────┐      ┌─────────────────────────────────┐  │
│  │ git-proxy-mcp    │      │ Your Git Configuration          │  │
│  │                  │◄────►│                                 │  │
│  │ • Auth callbacks │      │ • ~/.gitconfig                  │
│  │ • Object stream  │      │ • SSH keys (ssh-agent)          │  │
│  │ • No file storage│      │ • Credential helpers            │  │
│  └────────┬─────────┘      └─────────────────────────────────┘  │
│           │                                                     │
└───────────┼─────────────────────────────────────────────────────┘
            │
            │ Streaming: files/patches (NOT credentials)
            ▼
┌─────────────────────────────────────────────────────────────────┐
│  AI's VM (files live here, credentials don't)                   │
│                                                                 │
│  ┌──────────────────┐                                           │
│  │ /home/claude/    │  AI workflow (all local, no network):     │
│  │   repo/          │  • git checkout -b feature                │
│  │     .git/        │  • vim src/main.rs                        │
│  │     src/         │  • cargo test                             │
│  │     Cargo.toml   │  • git commit -m "fix bug"                │
│  └──────────────────┘                                           │
└─────────────────────────────────────────────────────────────────┘
```

### What Flows Where

| Data | Your PC | Network | AI's VM |
|------|---------|---------|--------|
| Credentials (PAT, SSH keys) | ✅ Stays | ❌ Never | ❌ Never |
| Repository files | ❌ Never stored | Streamed | ✅ Lives here |
| Git objects/history | ❌ Never stored | Streamed | ✅ Lives here |
| Commits/patches | ❌ Temporary only | Streamed | ✅ Created here |

---

## MCP Tools

### Tier 1: Single-Response Tools

#### `repo_clone`

Stream a repository to the AI's workspace (small-to-medium repos).

```json
{
    "name": "repo_clone",
    "arguments": {
        "url": "https://github.com/user/private-repo",
        "branch": "main",
        "depth": 1,
        "sparse": ["src/", "Cargo.toml"]
    }
}
```

**Optional arguments not shown above:**

- `exclude_binary` (bool) — skip binary files
- `max_file_size` (number, bytes) — skip files exceeding the size limit
- `resolve_lfs` (bool) — fetch and substitute LFS pointer files with their actual content
- `include_submodules` (bool) — recursively fetch submodules
- `submodule_depth` (number) — submodule recursion depth. Omit for unlimited (mirroring
  `git clone --recurse-submodules`). `1` = top-level submodules only; `0` = skip submodules
  entirely (overriding `include_submodules: true`).
- `submodule_include` (array of glob patterns) — only fetch submodules matching at least one pattern
- `submodule_exclude` (array of glob patterns) — skip submodules matching any pattern (takes precedence over include)

**Response:** Base64-encoded tar.gz `archive`, `commit` SHA, `branch`, `file_count`, `archive_size`
(bytes, before base64), and a `hint` string pointing at `helper_script` for extraction.
Optional counters appear only when non-zero: `skipped_by_filter`, `skipped_binary`,
`skipped_too_large`, `skipped_path_too_long`, `lfs_resolved`, `lfs_failed`,
`submodules_included`, `submodules_failed`.

#### `repo_push`

Push a git bundle from AI's workspace to remote.

```json
{
    "name": "repo_push",
    "arguments": {
        "url": "https://github.com/user/private-repo",
        "branch": "feature/fix-bug",
        "bundle": "<base64-encoded git bundle>",
        "force": false
    }
}
```

**Response:** `branch`, pushed `commit` SHA, `force` flag (echoed back), sanitised `remote_url`,
and a `hint` string explaining how to create bundles for follow-up pushes.

### Tier 2: Chunked Streaming Tools (Large Repos)

For repositories too large to transfer in a single response.

#### `repo_clone_start`

Start a chunked clone session.

```json
{
    "name": "repo_clone_start",
    "arguments": {
        "url": "https://gitlab.com/org/large-repo",
        "branch": "main",
        "depth": 1,
        "chunk_size": 1048576
    }
}
```

**Optional arguments not shown above** — same semantics as the corresponding `repo_clone` arguments documented above:

- `sparse` (array of paths/globs)
- `exclude_binary` (bool)
- `max_file_size` (number, bytes)
- `resolve_lfs` (bool)
- `include_submodules` (bool)
- `submodule_depth` (number)
- `submodule_include` (array of glob patterns)
- `submodule_exclude` (array of glob patterns)

**Response:** `session_id`, `total_chunks`, `total_size` (bytes, total archive size before base64),
`chunk_size` (bytes, the negotiated per-chunk size after clamping), `commit`, `branch`, `file_count`,
and a `hint` string. The same optional skipped/LFS/submodule counters as `repo_clone` appear when non-zero.

#### `repo_clone_chunk`

Get a chunk from a streaming session.

```json
{
    "name": "repo_clone_chunk",
    "arguments": {
        "session_id": "stream_abc123",
        "chunk_index": 0
    }
}
```

**Response:** Base64-encoded `data`, the `chunk_index` (echoed back), `chunk_size` (this chunk's
size in bytes before base64), `is_last` flag, and `next_missing_chunk` (omitted when no chunks
remain — used to resume after an interrupted transfer).

#### `repo_clone_status`

Check progress and resume state of a chunked clone session.

```json
{
    "name": "repo_clone_status",
    "arguments": {
        "session_id": "stream_abc123"
    }
}
```

**Response:** `session_id` (echoed back), `total_chunks`, `delivered_chunks`, `next_missing_chunk`
(null when all chunks have been retrieved), `progress_percent` (0.0–100.0), and `is_complete` flag.

#### `repo_clone_cancel`

Cancel a streaming session (optional, auto-expires after the configured timeout).

```json
{
    "name": "repo_clone_cancel",
    "arguments": {
        "session_id": "stream_abc123"
    }
}
```

**Response:** `{ "cancelled": <bool> }` — `true` if a session was found and removed, `false` if no such session existed (not an error).

### Other Tools

#### `repo_pull`

Sync new changes from remote to AI's workspace.

```json
{
    "name": "repo_pull",
    "arguments": {
        "url": "https://github.com/user/private-repo",
        "branch": "main",
        "since_commit": "abc123"
    }
}
```

**Response:** Unified `diff`, base64 tar.gz of changed/added files (`files_archive`), `changed_files` list with per-file change types,
`deleted_files` list, `base_commit` and `new_commit` SHAs, change `stats`, `up_to_date` flag, and a
`hint` string pointing at `helper_script` for extracting `files_archive`.

#### `repo_diff`

Get diff between two commits.

```json
{
    "name": "repo_diff",
    "arguments": {
        "url": "https://github.com/user/private-repo",
        "base_commit": "abc123",
        "head_commit": "def456"
    }
}
```

**Response:** Unified `diff` text, `stats` (additions/deletions/files-changed counts), and the
fully-resolved `base_commit` and `head_commit` SHAs (so the AI can cache the comparison without
re-resolving the original refs).

#### `repo_refs`

List remote branches and tags.

```json
{
    "name": "repo_refs",
    "arguments": {
        "url": "https://github.com/user/private-repo"
    }
}
```

**Response:** `branches` and `tags` lists (each entry has the ref name and commit SHA),
`default_branch` (e.g. `main` or `master`, taken from the remote HEAD), and `total_refs` count.

### Utilities

#### `helper_script`

Get a Python helper script for processing results (decoding base64, extracting tar.gz).

```json
{
    "name": "helper_script",
    "arguments": {}
}
```

**Response:** Python script source code, plus suggested `filename`, brief `usage` instructions, and the script `version`.

---

## Installation

### Prerequisites

#### Git CLI

The server invokes `git` for two operations: `git credential fill` (to read your stored
credentials via the OS credential helper — see `src/git2_ops/auth.rs`) and `git fetch` from a
bundle file (to apply a `repo_push` payload before the authenticated push — see
`src/git2_ops/push.rs`). Any reasonably modern git (2.x) on `PATH` works; bundles produced by
git ≥ 2.53 (with the `# v3 git bundle` header) are also accepted.

#### Git authentication

Configure Git to authenticate without prompting:

```bash
# macOS
git config --global credential.helper osxkeychain

# Windows
git config --global credential.helper manager

# Linux
git config --global credential.helper libsecret
```

For SSH, ensure your key is in ssh-agent:

```bash
eval "$(ssh-agent -s)"
ssh-add ~/.ssh/id_ed25519

# macOS: also cache the key's passphrase in the Keychain
ssh-add --apple-use-keychain ~/.ssh/id_ed25519
```

### Usage with Claude Desktop

Add to your Claude Desktop MCP configuration:

```json
{
    "mcpServers": {
        "git-proxy": {
            "command": "git-proxy-mcp",
            "args": []
        }
    }
}
```

---

## Configuration

Configuration file location:

- **Linux/macOS:** `~/.git-proxy-mcp/config.json`
- **Windows:** `%USERPROFILE%\.git-proxy-mcp\config.json`

```json
{
    "git_identity": {
        "name": "Claude AI",
        "email": "ai-assistant@your-domain.com"
    },
    "security": {
        "allow_force_push": false,
        "protected_branches": ["main", "master"]
    },
    "logging": {
        "level": "warn",
        "audit_log_path": "~/.git-proxy-mcp/audit.log"
    },
    "timeouts": {
        "request_timeout_secs": 300
    },
    "limits": {
        "max_output_bytes": 10485760
    },
    "rate_limits": {
        "max_burst": 20,
        "refill_rate_per_sec": 5.0
    },
    "proxy": {
        "url": "http://proxy.example.com:8080",
        "no_proxy": "*.internal.com,localhost"
    },
    "sessions": {
        "timeout_secs": 3600,
        "max_streaming_sessions": 10,
        "max_repo_sessions": 100
    },
    "lfs": {
        "retry_max_attempts": 3,
        "max_object_size": 104857600
    },
    "submodules": {
        "exclude_patterns": ["vendor/*"]
    }
}
```

For a fully-populated example showing every section and option, see [`config/example-config.json`](config/example-config.json).

### Configuration Options

| Section | Option | Description |
|---------|--------|-------------|
| `git_identity` | `name` | Name for AI-assisted commits, e.g. "Claude AI" (default: null — AI sets its own identity) |
| `git_identity` | `email` | Email for AI-assisted commits (default: null) |
| `security` | `allow_force_push` | Allow force pushes (default: false) |
| `security` | `protected_branches` | Branches that block force push and deletion. Default: empty list, which the server treats as "use the built-in safe set" (`main`, `master`, `develop`); set to any non-empty list to override the fallback. |
| `security` | `repo_allowlist` | Only allow these repo patterns (default: null — allowlist mode disabled) |
| `security` | `repo_blocklist` | Block these repo patterns (default: null — no blocklist) |
| `logging` | `level` | Log level: trace, debug, info, warn, error (default: `warn`) |
| `logging` | `audit_log_path` | Path to audit log file (default: null — audit logging disabled) |
| `timeouts` | `request_timeout_secs` | Git operation timeout (default: 300) |
| `limits` | `max_output_bytes` | Max combined stdout+stderr per command (default: 10 MiB) |
| `rate_limits` | `max_burst` | Max burst operations (default: 20) |
| `rate_limits` | `refill_rate_per_sec` | Sustained rate limit (default: 5.0) |
| `proxy` | `url` | Proxy URL — HTTP, HTTPS, or SOCKS5 (default: null — no proxy, falls back to git's `http.proxy`) |
| `proxy` | `no_proxy` | Comma-separated hosts to bypass proxy (default: null) |
| `sessions` | `timeout_secs` | Session inactivity timeout (default: 3600) |
| `sessions` | `max_streaming_sessions` | Max Tier 2 streaming sessions (default: 10) |
| `sessions` | `max_repo_sessions` | Max repo tracking sessions (default: 100) |
| `lfs` | `retry_max_attempts` | Max LFS download retries (default: 3) |
| `lfs` | `retry_initial_backoff_ms` | Initial retry backoff in ms (default: 500) |
| `lfs` | `retry_max_backoff_ms` | Maximum retry backoff in ms (default: 30000) |
| `lfs` | `retry_backoff_multiplier` | Exponential backoff multiplier (default: 2.0) |
| `lfs` | `max_object_size` | Max single LFS object size in bytes (default: null — unlimited; oversized objects are kept as pointer files) |
| `lfs` | `request_timeout_secs` | HTTP request timeout for the Batch API POST in seconds (default: 300) |
| `lfs` | `connect_timeout_secs` | HTTP connect timeout in seconds (default: 30) |
| `lfs` | `download_timeout_secs` | HTTP per-object download timeout in seconds — typically larger than `request_timeout_secs` for multi-GiB blobs (default: 600) |
| `submodules` | `max_concurrent` | Parallel submodule fetches (default: 4) |
| `submodules` | `max_failures` | Max submodule failures before stopping (default: 3) |
| `submodules` | `include_patterns` | Glob patterns to include (default: null — all submodules allowed) |
| `submodules` | `exclude_patterns` | Glob patterns to exclude (default: null — nothing excluded) |

### Validation

The configuration is validated when it loads; an invalid value aborts startup
with a `configuration validation failed: …` message naming the offending field.
The checks reject only values that would render a subsystem unusable: a zero
`timeouts.request_timeout_secs` or any of the `lfs.*_timeout_secs` (every request
would time out immediately), a zero `limits.max_output_bytes` (every command's
combined stdout+stderr would be truncated to nothing), `rate_limits.max_burst`
of `0` (every operation blocked forever), a non-finite or negative
`rate_limits.refill_rate_per_sec`
(`0.0` is allowed — it means "burst once, never refill"), a zero
`sessions.timeout_secs` / `sessions.max_streaming_sessions` /
`sessions.max_repo_sessions`, and a `logging.level` outside
`trace`/`debug`/`info`/`warn`/`error`.

---

## Contributing

Contributions welcome! See [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines.

- Follow the style guide in [STYLE.md](STYLE.md)
- Security issues: see [SECURITY.md](SECURITY.md)

---

## Licence

Copyright (C) 2026 Matej Gomboc <https://github.com/MatejGomboc/git-proxy-mcp>.

GNU General Public License v3.0 — see [LICENCE](LICENCE).

---

## Links

- [MCP Specification](https://modelcontextprotocol.io/)
- [git2 (libgit2 Rust bindings)](https://crates.io/crates/git2)
- [Report an Issue](https://github.com/MatejGomboc/git-proxy-mcp/issues)
