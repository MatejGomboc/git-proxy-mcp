# Vision: Credential Relay for Cloud AI

This document describes the architectural vision for git-proxy-mcp.

## The Problem

Cloud-based AI assistants (Claude.ai, ChatGPT, Gemini) have:

- Full Linux VMs with compute capability
- Ability to run git, build code, run tests
- **No access to user's credentials for private repos**

## The Solution: Credential Relay

```text
┌─────────────────────────────────────────────────────────────────────────┐
│  CREDENTIAL RELAY ARCHITECTURE                                          │
│                                                                         │
│  GitHub                User's PC                      AI's VM           │
│    │                      │                              │              │
│    │◄──── credentials ────┤                              │              │
│    │      (SSH/PAT)       │                              │              │
│    │                      │                              │              │
│    │── repo contents ────►│──── repo contents ──────────►│              │
│    │   (authenticated)    │    (NO credentials!)         │              │
│    │                      │                              │              │
│    │                      │◄─── changes ─────────────────┤              │
│    │◄── push (with creds)─┤    (patches, no creds)       │              │
│    │                      │                              │              │
└─────────────────────────────────────────────────────────────────────────┘

Credentials: NEVER leave user's PC
Repo files:  Stream through MCP → land in AI's VM
```

**Key Principle:** The MCP server acts as an authenticated relay. Credentials stay local. Only file contents flow to the AI.

---

## Two Implementation Tiers

### Tier 1: Single-Response Streaming

```text
GitHub ──► MCP (buffer in RAM) ──► AI
```

| Property | Value |
|----------|-------|
| Files on user's disk | No |
| Memory usage | O(repo size) |
| Large repo support | Limited |
| Complexity | Low |
| Tools | `repo/clone`, `repo/push` |

**Use case:** Small to medium repos.

### Tier 2: Chunked Streaming

```text
GitHub ──► MCP (small chunks) ──► AI
```

| Property | Value |
|----------|-------|
| Files on user's disk | No |
| Memory usage | O(chunk size) — constant |
| Large repo support | Yes |
| Complexity | Medium |
| Tools | `repo/clone_start`, `repo/clone_chunk`, `repo_clone_status`, `repo/clone_cancel` |

**Use case:** Any repo size, production-ready.

---

## Security Model

### What Stays on User's PC

- Personal Access Tokens (in OS credential store)
- SSH private keys (in ssh-agent)
- All authentication secrets
- Git credential helper configuration

### What Flows to AI's VM

- Repository file contents
- Git object data (commits, trees, blobs)
- Branch and tag metadata
- Diff/patch data for pushes

### What NEVER Flows to AI

- Credentials of any kind
- Tokens (even short-lived ones)
- SSH keys or signatures
- Authentication headers

---

## Data Flow: Clone Operation

```text
┌─────────────────────────────────────────────────────────────────────────────┐
│ CLONE: Authenticated fetch → stream to AI                                   │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                             │
│  GitHub                 User's PC                        AI's VM            │
│    │                       │                                │               │
│    │  git2 fetch           │                                │               │
│    │  (with credentials)   │                                │               │
│    ├──────────────────────►│                                │               │
│    │                       │                                │               │
│    │                       │  Objects stored in             │               │
│    │                       │  BARE REPO (temp, no checkout) │               │
│    │                       │                                │               │
│    │                       │  Stream tree to tar.gz         │               │
│    │                       │  (in memory or chunked)        │               │
│    │                       │                                │               │
│    │                       │  MCP response                  │               │
│    │                       ├───────────────────────────────►│               │
│    │                       │  (file contents only)          │               │
│    │                       │                                │               │
│    │                       │                                │  Extract      │
│    │                       │                                │  git init     │
│    │                       │                                │  Full repo!   │
│    │                       │                                │               │
│    │                       │  Clean up temp                 │               │
│    │                       │                                │               │
└─────────────────────────────────────────────────────────────────────────────┘
```

## Data Flow: Push Operation

```text
┌─────────────────────────────────────────────────────────────────────────────┐
│ PUSH: Receive changes from AI → authenticated push                          │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                             │
│  AI's VM                   User's PC                        GitHub          │
│    │                          │                                │            │
│    │  Create git bundle       │                                │            │
│    │  (commits to push)       │                                │            │
│    │                          │                                │            │
│    │  MCP request             │                                │            │
│    ├─────────────────────────►│                                │            │
│    │  (bundle, no creds)      │                                │            │
│    │                          │                                │            │
│    │                          │  Unbundle to temp repo         │            │
│    │                          │  Validate (guards, security)   │            │
│    │                          │                                │            │
│    │                          │  git2 push                     │            │
│    │                          ├───────────────────────────────►│            │
│    │                          │  (with credentials)            │            │
│    │                          │                                │            │
│    │                          │  Clean up temp                 │            │
│    │                          │                                │            │
│    │  MCP response            │                                │            │
│    │◄─────────────────────────┤                                │            │
│    │  (commit URL)            │                                │            │
│    │                          │                                │            │
└─────────────────────────────────────────────────────────────────────────────┘
```

---

## MCP Tools

| Tool | Description |
|------|-------------|
| `repo/clone` | Authenticated fetch, stream repo as tar.gz |
| `repo/push` | Receive bundle from AI, authenticated push |
| `repo/clone_start` | Start chunked clone for large repos |
| `repo/clone_chunk` | Get chunk from streaming session |
| `repo_clone_status` | Check progress and resume state of streaming session |
| `repo/clone_cancel` | Cancel streaming session |
| `repo/pull` | Stream delta of changes since last sync |
| `repo/diff` | Get diff between commits |
| `repo/refs` | List branches and tags |
| `helper_script` | Get Python utility script for processing results |

---

## Design Principles

1. **Credentials never leave** — Not even "safe" short-lived tokens
2. **No file storage** — Temp bare repos only, cleaned immediately
3. **Stream, don't buffer** — Chunked transfer for large repos (Tier 2)
4. **Validate everything** — Security guards on push operations
5. **Audit everything** — Log all operations (without credentials)
