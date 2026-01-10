# Architecture: git-proxy-mcp

This document describes the architecture — a pure streaming credential proxy.

## Core Principle: ZERO FILE STORAGE

**The MCP server NEVER stores repository files on the user's disk.**

This is the fundamental design principle that distinguishes git-proxy-mcp from other approaches:

| Approach | Files on User's Disk |
|----------|---------------------|
| Git CLI | Full working tree |
| **git-proxy-mcp** | **NONE** |

## How Zero-Storage Works

### Clone Flow (In Detail)

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ CLONE: Stream directly from git objects to tar archive                      │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                             │
│  GitHub                 User's PC                        AI's VM            │
│    │                       │                                │               │
│    │  git2 fetch           │                                │               │
│    │  (pack protocol)      │                                │               │
│    ├──────────────────────►│                                │               │
│    │                       │                                │               │
│    │                       │  Objects stored in             │               │
│    │                       │  BARE REPO (no checkout)       │               │
│    │                       │  Temp dir, auto-cleaned        │               │
│    │                       │                                │               │
│    │                       │  Walk tree:                    │               │
│    │                       │  for each blob:                │               │
│    │                       │    read content from objects   │               │
│    │                       │    write to tar (in memory)    │               │
│    │                       │                                │               │
│    │                       │  NO working tree checkout      │               │
│    │                       │  NO source files on disk       │               │
│    │                       │                                │               │
│    │                       │  MCP response                  │               │
│    │                       ├───────────────────────────────►│               │
│    │                       │  (base64 tar.gz)               │               │
│    │                       │                                │               │
│    │                       │                                │  Extract      │
│    │                       │                                │  git init     │
│    │                       │                                │  Full repo!   │
│    │                       │                                │               │
└─────────────────────────────────────────────────────────────────────────────┘
```

### The Key Insight: Bare Repositories

A **bare repository** contains:

- Git objects (commits, trees, blobs) — compressed, deduplicated
- References (branches, tags)
- NO working tree (no checked-out files)

We use bare repos because:

1. `git2::Repository::init_bare()` — no working directory created
2. We can fetch objects from remotes
3. We can walk trees and read blob contents
4. We NEVER checkout — no source files written

```rust
// This is what we do:
let repo = Repository::init_bare(temp_path)?;  // No working tree
let tree = commit.tree()?;
for entry in tree.iter() {
    let blob = repo.find_blob(entry.id())?;
    // blob.content() gives us bytes directly from object DB
    // We write these to tar, not to disk
}

// This is what we DON'T do:
let repo = Repository::clone(url, path)?;  // Would checkout files!
repo.checkout_head(...)?;                  // Would write files!
```

### Push Flow

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ PUSH: Receive bundle, push to remote                                        │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                             │
│  AI's VM                   User's PC                        GitHub          │
│    │                          │                                │            │
│    │  git bundle create       │                                │            │
│    │  (in AI's repo)          │                                │            │
│    │                          │                                │            │
│    │  MCP request             │                                │            │
│    ├─────────────────────────►│                                │            │
│    │  (base64 bundle)         │                                │            │
│    │                          │                                │            │
│    │                          │  Create bare temp repo         │            │
│    │                          │  Unbundle (adds objects)       │            │
│    │                          │  NO checkout                   │            │
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

## What Touches Disk (And What Doesn't)

### On User's Disk

| Data | Written to Disk? | Notes |
|------|-----------------|-------|
| Source files | **NO** | Never checked out |
| Git objects | Temp only | Bare repo, deleted after |
| Bundle file | Temp only | For unbundle operation |
| Credentials | **NO** | System helpers only |
| Tar archive | **NO** | Built in memory |

### Temp Directory Contents

```
/tmp/git-proxy-xxxxx/        # Temp dir (auto-deleted)
├── objects/                 # Git object database
│   ├── pack/               # Pack files from fetch
│   └── ...                 # Loose objects
├── refs/                   # Branch references
├── HEAD                    # Current ref
└── config                  # Bare repo config

NO src/, NO Cargo.toml, NO working tree files!
```

## Memory vs Disk Trade-offs

| Operation | Memory Usage (Tier 1) | Memory Usage (Tier 2) | Disk Usage |
|-----------|----------------------|----------------------|------------|
| Fetch objects | O(packed size) | O(packed size) | Temp only |
| Walk tree | O(1) per entry | O(1) per entry | None |
| Build tar | O(archive size) | O(archive size) | None |
| Encode base64 | O(archive size) | O(chunk size) | None |

## Tier 2: Chunked Streaming

For large repositories, Tier 1 may buffer too much data in memory. Tier 2 solves this with a multi-call protocol:

### Chunked Clone Flow

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ TIER 2: Chunked streaming for large repos                                   │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                             │
│  AI's VM                      User's PC                       GitHub        │
│    │                             │                               │          │
│    │  repo/clone_start           │                               │          │
│    ├────────────────────────────►│  fetch + tar (in memory)      │          │
│    │                             │◄──────────────────────────────┤          │
│    │◄────────────────────────────┤  session_id, total_chunks     │          │
│    │                             │                               │          │
│    │  repo/clone_chunk(0)        │                               │          │
│    ├────────────────────────────►│                               │          │
│    │◄────────────────────────────┤  base64 chunk 0               │          │
│    │                             │                               │          │
│    │  repo/clone_chunk(1)        │                               │          │
│    ├────────────────────────────►│                               │          │
│    │◄────────────────────────────┤  base64 chunk 1               │          │
│    │                             │                               │          │
│    │        ...                  │                               │          │
│    │                             │                               │          │
│    │  repo/clone_chunk(N)        │                               │          │
│    ├────────────────────────────►│                               │          │
│    │◄────────────────────────────┤  base64 chunk N, is_last=true │          │
│    │                             │  Session auto-cleaned         │          │
│    │                             │                               │          │
│    │  Concatenate chunks         │                               │          │
│    │  Extract tar.gz             │                               │          │
│    │  Full repo!                 │                               │          │
│                                                                             │
└─────────────────────────────────────────────────────────────────────────────┘
```

### Benefits of Chunked Streaming

| Aspect | Tier 1 | Tier 2 |
|--------|--------|--------|
| Response size | Entire repo | Configurable (1KB-4MB) |
| Resume on failure | Start over | Resume from last chunk |
| Progress reporting | None | chunk_index / total_chunks |
| Memory per response | O(repo) | O(chunk) |
| Session management | None | Session ID with timeout |

## Security Implications

### Credential Security

Credentials never stored, git2 callbacks only.

### Data Security

**Key security benefits:**

- Source files never on user's disk, even temporarily
- If MCP server crashes, no source files left behind
- Nothing to clean up, nothing to leak

### Audit Trail

All operations logged (without credentials), showing:

- Repository URL (sanitized)
- Branch
- Commit SHA
- Operation success/failure
- Duration

## Component Overview

```
src/
├── git2_ops/                    # git2 operations
│   ├── auth.rs                  # Credential callbacks (multi-provider)
│   ├── clone.rs                 # Bare fetch + tree streaming
│   ├── push.rs                  # Bundle processing + push
│   └── error.rs                 # Error types with credential sanitisation
│
├── streaming/                   # Transfer formats
│   ├── tar.rs                   # Tree → tar.gz (in memory)
│   ├── bundle.rs                # Git bundle handling
│   └── chunked.rs               # Tier 2: Session manager + chunking
│
├── mcp/tools/                   # MCP tool handlers
│   ├── repo_clone.rs            # Tier 1: repo/clone
│   ├── repo_push.rs             # Tier 1: repo/push
│   ├── repo_clone_start.rs      # Tier 2: repo/clone_start
│   └── repo_clone_chunk.rs      # Tier 2: repo/clone_chunk + cancel
│
├── session.rs                   # Session tracking (no files!)
│
└── security/                    # Guards
    ├── audit.rs                 # Operation audit logging
    ├── guards.rs                # Branch + push guards + repo filter
    └── rate_limit.rs            # Token bucket rate limiting
```

## Supported Git Providers

The credential system uses standard git protocols, supporting:

| Provider | HTTPS | SSH | Notes |
|----------|-------|-----|-------|
| GitHub | ✅ | ✅ | github.com |
| GitLab | ✅ | ✅ | gitlab.com + self-hosted |
| Bitbucket | ✅ | ✅ | bitbucket.org |
| Azure DevOps | ✅ | ✅ | dev.azure.com |
| Self-hosted | ✅ | ✅ | Gitea, Gogs, etc. |

All providers work automatically via:

- **SSH**: ssh-agent (private key never leaves agent)
- **HTTPS**: System credential helpers (macOS Keychain, Windows Credential Manager, libsecret)
