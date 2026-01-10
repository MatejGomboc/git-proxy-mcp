# Architecture: git-proxy-mcp v2

This document describes the v2 architecture — a pure streaming credential proxy.

## Core Principle: ZERO FILE STORAGE

**The MCP server NEVER stores repository files on the user's disk.**

This is the fundamental design principle that distinguishes git-proxy-mcp from other approaches:

| Approach | Files on User's Disk |
|----------|---------------------|
| Git CLI | Full working tree |
| v1 git-proxy-mcp | Full clone (then deleted) |
| **v2 git-proxy-mcp** | **NONE** |

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

| Operation | Memory Usage | Disk Usage |
|-----------|-------------|------------|
| Fetch objects | O(packed size) | Temp only |
| Walk tree | O(1) per entry | None |
| Build tar | O(archive size) | None |
| Encode base64 | O(archive size) | None |

For very large repos, we may need streaming/chunking to avoid OOM.

## Security Implications

### Credential Security

Same as before — credentials never stored, git2 callbacks only.

### Data Security

**Stronger than v1:**

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
│   ├── auth.rs                  # Credential callbacks
│   ├── clone.rs                 # Bare fetch + tree streaming
│   └── push.rs                  # Bundle processing + push
│
├── streaming/                   # Transfer formats
│   ├── tar.rs                   # Tree → tar.gz (in memory)
│   └── bundle.rs                # Git bundle handling
│
├── mcp/tools/                   # MCP tool handlers
│   ├── repo_clone.rs            # repo/clone
│   ├── repo_push.rs             # repo/push
│   └── repo_pull.rs             # repo/pull
│
├── session.rs                   # Session tracking (no files!)
│
└── security/                    # Guards (from v1)
    ├── branch_guard.rs
    ├── push_guard.rs
    └── rate_limiter.rs
```

## Comparison: v1 vs v2

| Aspect | v1 | v2 |
|--------|----|----|  
| Git library | CLI subprocess | git2 (in-process) |
| Clone method | Full checkout | Bare repo, tree walk |
| Files on disk | Yes (temp) | No (objects only) |
| Working tree | Created, then deleted | Never created |
| Tar creation | From disk files | From git objects |
| Memory usage | Lower | Higher |
| Security | Good | Better (no files) |
