# git-proxy-mcp — AI Assistant Context

## What Is This Project?

A secure credential proxy that enables **cloud-based AI assistants** (Claude.ai, ChatGPT, Gemini) to work with private Git repositories.

**The key insight:** Cloud AIs have their own VMs with full compute capability. They can run git, build code, run tests. They just can't authenticate to private repos.

**We solve exactly that:** Stream Git data through an authenticated proxy. **No files stored on user's machine.**

## Architecture (v2) — ZERO FILE STORAGE

```
GitHub/GitLab              User's PC                  AI's VM
     │                         │                         │
     │  git2 fetch (bare)      │                         │
     ├────────────────────────►│                         │
     │                         │                         │
     │                    Walk tree,                     │
     │                    stream blobs                   │
     │                    directly to tar                │
     │                    (IN MEMORY)                    │
     │                         │                         │
     │                         │  MCP response           │
     │                         ├────────────────────────►│
     │                         │  (tar.gz stream)        │
     │                         │                         │
     │                    NO SOURCE FILES                │
     │                    ON USER'S DISK                 │
```

**Critical design principle:** The MCP server NEVER writes repository source files to the user's disk. We use bare repositories and stream blob contents directly from git's object database to an in-memory tar archive.

## Quick Reference

| Resource | Location |
|----------|----------|
| Battle plan | `TODO.md` (very detailed!) |
| Architecture details | `docs/ARCHITECTURE.md` |
| AI workflow examples | `docs/AI_WORKFLOW.md` |
| Security model | `docs/SECURITY.md` |
| Style guide | `STYLE.md` |

## Critical Rules

### 🔴 NEVER Do These

1. **NEVER store credentials** — Not in memory, not in config, not in session state
2. **NEVER log credentials** — No Cred objects, no tokens, no keys
3. **NEVER checkout working tree** — Use bare repos only
4. **NEVER write source files to disk** — Stream to tar in memory
5. **NEVER push to main** — Always feature branch + PR

### 🟢 ALWAYS Do These

1. **Use `Repository::init_bare()`** — No working tree
2. **Use `repo.find_blob().content()`** — Read from object DB
3. **Use `tar::Builder::new(Vec::new())`** — Build in memory
4. **Use git2 credential callbacks** — System helpers only
5. **Use `TempDir`** — Auto-cleanup on drop

## Key Implementation Patterns

### ZERO-STORAGE Clone Pattern

```rust
// ✅ CORRECT: Bare repo, walk tree, stream blobs
pub fn create_tar_from_tree(repo: &Repository, commit_id: Oid) -> Result<Vec<u8>> {
    let commit = repo.find_commit(commit_id)?;
    let tree = commit.tree()?;
    
    // Build tar in memory
    let mut buffer = Vec::new();
    let encoder = GzEncoder::new(&mut buffer, Compression::fast());
    let mut tar = tar::Builder::new(encoder);
    
    tree.walk(TreeWalkMode::PreOrder, |dir, entry| {
        if let Some(ObjectType::Blob) = entry.kind() {
            // Read blob directly from object database
            let blob = repo.find_blob(entry.id())?;
            let content = blob.content();  // NO disk read!
            
            // Write to tar in memory
            tar.append_data(&mut header, path, content)?;
        }
        TreeWalkResult::Ok
    })?;
    
    Ok(buffer)
}

// ❌ WRONG: Would write files to disk
let repo = Repository::clone(url, path)?;  // Creates working tree!
repo.checkout_head(...)?;                   // Writes files!
let files = std::fs::read_dir(path)?;       // Reading disk files!
```

### Credential Handling

```rust
// ✅ CORRECT: System helpers via callbacks
callbacks.credentials(|url, username, allowed| {
    if allowed.contains(CredentialType::SSH_KEY) {
        return Cred::ssh_key_from_agent(username.unwrap_or("git"));
    }
    if allowed.contains(CredentialType::USER_PASS_PLAINTEXT) {
        let config = git2::Config::open_default()?;
        return Cred::credential_helper(&config, url, username);
    }
    Err(git2::Error::from_str("no credential method"))
});

// ❌ WRONG
let token = "ghp_xxxx";           // NEVER hardcode
log::debug!("Cred: {:?}", cred);  // NEVER log
session.credential = cred;        // NEVER store
```

## Project Structure

```
src/
├── git2_ops/              # git2 library integration
│   ├── mod.rs
│   ├── auth.rs            # Credential callbacks (CRITICAL)
│   ├── clone.rs           # Bare fetch + tree streaming
│   ├── push.rs            # Bundle processing + push
│   └── error.rs
│
├── streaming/             # In-memory transfer formats
│   ├── mod.rs
│   ├── tar.rs             # Tree → tar.gz (NO DISK)
│   └── bundle.rs          # Git bundle handling
│
├── mcp/
│   ├── server.rs          # Tool dispatch
│   └── tools/
│       ├── repo_clone.rs  # repo/clone handler
│       ├── repo_push.rs   # repo/push handler
│       └── repo_pull.rs   # repo/pull handler
│
├── session.rs             # Session tracking (NO files!)
│
└── security/              # Guards (from v1)
```

## What Gets Written to Disk?

| Data | Disk? | Notes |
|------|-------|-------|
| Source files | **NO** | Never checked out |
| Git objects | Temp | Bare repo, auto-deleted |
| Bundle file | Temp | For unbundle, auto-deleted |
| Tar archive | **NO** | Built in memory |
| Credentials | **NO** | System helpers only |

## Current Development Phase

**Phase 1: Foundation Rewrite** ← WE ARE HERE

See `TODO.md` for extremely detailed implementation steps including:
- Exact code patterns to use
- Files to create
- Acceptance criteria
- What to verify (especially: NO DISK WRITES)

## Testing

```bash
# Unit tests
cargo test

# Integration tests (need credentials)
GIT_TEST_REPO_URL=https://github.com/you/test cargo test --features integration

# CRITICAL: Verify no disk writes
# On Linux:
strace -f -e write cargo run ... 2>&1 | grep -v /tmp
# On macOS:
fs_usage -w cargo run ...
```

## Common Mistakes to Avoid

| Mistake | Why It's Wrong | Correct Approach |
|---------|---------------|------------------|
| `Repository::clone()` | Creates working tree | `Repository::init_bare()` + fetch |
| `repo.checkout_head()` | Writes files | Don't checkout, walk tree instead |
| `std::fs::read_dir()` | Reading disk files | Use `tree.walk()` + `find_blob()` |
| `std::fs::write()` | Writing files | Write to `Vec<u8>` |
| Storing `Cred` | Credential leak | Use callback, don't store |

## Off Limits

**`CODE_OF_CONDUCT.md`** — Do not modify. Owned by repository owner.
