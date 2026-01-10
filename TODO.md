# TODO — Development Battle Plan

## Overview

**Goal:** Build a secure credential proxy that enables cloud-based AI assistants to work with private Git repositories. The AI maintains a full local repo in its VM; the MCP server only handles authentication and streaming.

**Target Users:** Cloud AI assistants with compute capability but no credential access:

- Claude.ai (with computer use)
- ChatGPT with Code Interpreter  
- Gemini with code execution
- Any sandboxed AI environment

**Non-Targets:** Local AI tools (Claude Code, Cursor, Aider) — they already have direct Git access.

**The Core Insight:**

```
Cloud AIs have:     ✅ Linux VM    ✅ Can run git    ❌ No credentials
We provide:         Authenticated streaming proxy (NO file storage)
Result:             AI works on private repos like a local developer
```

---

## Architecture: Pure Credential Proxy (Zero Storage)

```
GIT PROVIDER                 YOUR PC                    AI's VM
(GitHub/GitLab)             (MCP Server)              (Claude.ai)

┌──────────────┐      ┌─────────────────┐      ┌─────────────────┐
│              │      │ git-proxy-mcp   │      │                 │
│ repo objects │◄────►│                 │◄────►│ /home/claude/   │
│              │      │ • Credentials   │      │   repo/         │
│              │      │ • git2 library  │      │   .git/         │
│              │      │ • ZERO storage  │      │   (full repo)   │
│              │      │ • Stream-through│      │                 │
└──────────────┘      └─────────────────┘      └─────────────────┘
                             │                        │
                      Credentials stay          Files live
                      on YOUR machine           in AI's VM
                      NO FILES stored           ONLY place files exist
```

### Critical Design Principle: NO FILE DUPLICATION

**The MCP server NEVER writes repository files to disk.** Everything is streamed:

| Operation | What Happens |
|-----------|-------------|
| Clone | git2 fetches objects → walk tree in memory → stream tar directly → AI receives |
| Push | AI sends bundle → git2 processes in memory → push to remote |
| Pull | git2 fetches delta → stream changes directly → AI receives |

**Why this matters:**
- User's disk is never cluttered with repo copies
- No cleanup needed
- No risk of leftover sensitive files
- Pure proxy behavior

---

## Design Decisions (v2)

| Decision | Choice | Rationale |
|----------|--------|----------|
| Git library | `git2` (libgit2) | In-process, streaming capable, credential callbacks |
| File storage on MCP | **ZERO** | Pure stream-through proxy |
| Clone method | Bare repo + tree walk | No working tree checkout |
| Transfer format | tar.gz built in memory | Stream blobs directly to tar builder |
| Push method | Receive bundle, process in memory | No disk writes |
| Session state | Stateful | Track repos to enable incremental sync |
| Credential handling | System helpers via git2 | Use existing git config, never store |

---

## Phase 1: Foundation Rewrite ← CURRENT

### 1.1 Add git2 Dependency and Module Structure

**Goal:** Set up git2 and create the module skeleton.

**Files to create/modify:**

```
Cargo.toml                 # Add git2 dependency
src/
├── git2_ops/              # NEW: git2 operations module
│   ├── mod.rs             # Module exports
│   ├── auth.rs            # Credential callbacks
│   ├── clone.rs           # Fetch + stream (NO checkout)
│   ├── push.rs            # Receive and push
│   └── error.rs           # git2-specific errors
└── streaming/             # NEW: in-memory transfer handling
    ├── mod.rs
    ├── tar.rs             # In-memory tar creation from tree
    └── bundle.rs          # Git bundle handling
```

**Cargo.toml additions:**

```toml
[dependencies]
git2 = "0.19"              # libgit2 bindings
flate2 = "1.0"             # gzip compression (in memory)
tar = "0.4"                # tar archive creation (in memory)
```

**Acceptance criteria:**

- [ ] `cargo build` succeeds with git2
- [ ] `src/git2_ops/mod.rs` exists with submodule declarations
- [ ] `src/streaming/mod.rs` exists with submodule declarations
- [ ] Basic smoke test: can create a `git2::Repository` object

---

### 1.2 Implement Credential Callbacks

**Goal:** Authentication via system credential helpers (no credential storage).

**File:** `src/git2_ops/auth.rs`

**Key concepts:**

```rust
use git2::{Cred, CredentialType, RemoteCallbacks};

/// Create callbacks that use system credential helpers.
/// CRITICAL: Never store, log, or transmit credentials.
pub fn create_callbacks() -> RemoteCallbacks<'static> {
    let mut callbacks = RemoteCallbacks::new();
    
    callbacks.credentials(|url, username_from_url, allowed_types| {
        // Try SSH agent first (if allowed)
        if allowed_types.contains(CredentialType::SSH_KEY) {
            if let Some(username) = username_from_url {
                // Use SSH agent - keys never leave the agent
                return Cred::ssh_key_from_agent(username);
            }
        }
        
        // Fall back to credential helper (for HTTPS)
        if allowed_types.contains(CredentialType::USER_PASS_PLAINTEXT) {
            let config = git2::Config::open_default()?;
            return Cred::credential_helper(&config, url, username_from_url);
        }
        
        Err(git2::Error::from_str("no suitable credential method"))
    });
    
    callbacks
}
```

**Important security notes:**

1. `Cred::credential_helper()` invokes the user's configured helper (e.g., `osxkeychain`, `manager`, `libsecret`)
2. `Cred::ssh_key_from_agent()` uses the running ssh-agent — private key never leaves the agent
3. NEVER log the `Cred` object or any intermediate auth values
4. NEVER store credentials in session state

**Acceptance criteria:**

- [ ] `create_callbacks()` function exists and compiles
- [ ] SSH agent authentication path implemented
- [ ] Credential helper path implemented
- [ ] No credentials in any log output (audit the code)
- [ ] Error messages don't leak credential details

---

### 1.3 Implement Streaming Clone (repo/clone tool) — ZERO DISK WRITES

**Goal:** Fetch a remote repo and stream its contents to the AI WITHOUT writing files to disk.

**This is the critical piece — we stream directly from git objects to tar:**

#### 1.3.1 Fetch Objects (Bare Repository)

**File:** `src/git2_ops/clone.rs`

```rust
use git2::{Repository, FetchOptions, RemoteCallbacks, Oid};
use std::path::Path;

pub struct FetchResult {
    pub repo: Repository,      // Bare repo with objects
    pub head_commit: Oid,
    pub branch: String,
}

/// Fetch repository objects WITHOUT creating a working tree.
/// Uses a bare repository — no files are checked out to disk.
pub fn fetch_repo(
    url: &str,
    branch: Option<&str>,
    callbacks: RemoteCallbacks,
) -> Result<FetchResult, Error> {
    // Create a bare repository (NO working directory)
    let temp_dir = tempfile::tempdir()?;
    let repo = Repository::init_bare(temp_dir.path())?;
    
    // Add remote and fetch
    let mut remote = repo.remote_anonymous(url)?;
    let mut fetch_opts = FetchOptions::new();
    fetch_opts.remote_callbacks(callbacks);
    
    // Fetch the specific branch or all
    let refspec = branch
        .map(|b| format!("refs/heads/{b}:refs/heads/{b}"))
        .unwrap_or_else(|| "refs/heads/*:refs/heads/*".to_string());
    
    remote.fetch(&[&refspec], Some(&mut fetch_opts), None)?;
    
    // Find HEAD commit
    let branch_name = branch.unwrap_or("main");
    let reference = repo.find_reference(&format!("refs/heads/{branch_name}"))?;
    let head_commit = reference.peel_to_commit()?.id();
    
    Ok(FetchResult {
        repo,
        head_commit,
        branch: branch_name.to_string(),
    })
}
```

**Key points:**
- `Repository::init_bare()` — NO working tree, just object database
- We fetch git objects only
- No `checkout` operation = no files written
- The temp_dir contains only `.git` internals, not source files

#### 1.3.2 Stream Tree to Tar (In-Memory)

**File:** `src/streaming/tar.rs`

```rust
use flate2::write::GzEncoder;
use flate2::Compression;
use git2::{Repository, Oid, ObjectType, TreeWalkMode, TreeWalkResult};
use std::io::Write;

/// Create a tar.gz archive by walking the git tree and streaming blobs.
/// 
/// CRITICAL: This does NOT checkout files to disk. We read blob contents
/// directly from git's object database and write them to the tar archive
/// in memory.
pub fn create_tar_from_tree(
    repo: &Repository,
    commit_id: Oid,
) -> Result<Vec<u8>, Error> {
    let commit = repo.find_commit(commit_id)?;
    let tree = commit.tree()?;
    
    // Build tar.gz entirely in memory
    let mut archive_buffer = Vec::new();
    {
        let encoder = GzEncoder::new(&mut archive_buffer, Compression::fast());
        let mut tar = tar::Builder::new(encoder);
        
        // Walk the tree and add each blob directly to tar
        tree.walk(TreeWalkMode::PreOrder, |dir, entry| {
            let path = if dir.is_empty() {
                entry.name().unwrap_or("").to_string()
            } else {
                format!("{}/{}", dir, entry.name().unwrap_or(""))
            };
            
            match entry.kind() {
                Some(ObjectType::Blob) => {
                    // Get blob content directly from object database
                    if let Ok(blob) = repo.find_blob(entry.id()) {
                        let content = blob.content();
                        
                        // Create tar header
                        let mut header = tar::Header::new_gnu();
                        header.set_path(&path).ok();
                        header.set_size(content.len() as u64);
                        header.set_mode(entry.filemode() as u32);
                        header.set_cksum();
                        
                        // Add to tar — blob content goes directly, no disk involved
                        tar.append(&header, content).ok();
                    }
                }
                Some(ObjectType::Tree) => {
                    // Directory entry — tar handles this automatically
                }
                _ => {}
            }
            
            TreeWalkResult::Ok
        })?;
        
        tar.finish()?;
    }
    
    Ok(archive_buffer)
}
```

**Key points:**
- `repo.find_blob(id)` — reads content from git object database
- `blob.content()` — raw bytes, never written to disk
- `tar::Builder::new(encoder)` — writes to `Vec<u8>` in memory
- The entire operation is: git objects → memory → tar bytes
- **ZERO files written to user's disk**

#### 1.3.3 MCP Tool Handler

**File:** `src/mcp/tools/repo_clone.rs`

```rust
use serde::{Deserialize, Serialize};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};

#[derive(Deserialize)]
pub struct RepoCloneArgs {
    pub url: String,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub depth: Option<u32>,
    #[serde(default)]
    pub sparse: Option<Vec<String>>,
}

#[derive(Serialize)]
pub struct RepoCloneResult {
    /// Base64-encoded tar.gz archive (built entirely in memory)
    pub archive: String,
    /// Commit SHA that was fetched
    pub commit: String,
    /// Branch name
    pub branch: String,
    /// Number of files in archive
    pub file_count: usize,
    /// Archive size in bytes (before base64)
    pub archive_size: usize,
}

pub async fn handle_repo_clone(args: RepoCloneArgs) -> Result<RepoCloneResult, ToolError> {
    // 1. Create auth callbacks
    let callbacks = crate::git2_ops::auth::create_callbacks();
    
    // 2. Fetch to bare repo (no working tree)
    let fetch_result = crate::git2_ops::clone::fetch_repo(
        &args.url,
        args.branch.as_deref(),
        callbacks,
    )?;
    
    // 3. Create tar.gz by walking tree (in memory, no disk writes)
    let archive_data = crate::streaming::tar::create_tar_from_tree(
        &fetch_result.repo,
        fetch_result.head_commit,
    )?;
    
    // 4. Encode as base64 for JSON transport
    let archive_base64 = BASE64.encode(&archive_data);
    
    // 5. The bare repo temp dir is cleaned up automatically
    
    Ok(RepoCloneResult {
        archive: archive_base64,
        commit: fetch_result.head_commit.to_string(),
        branch: fetch_result.branch,
        file_count: count_entries(&archive_data)?,
        archive_size: archive_data.len(),
    })
}
```

#### 1.3.4 Update MCP Tool Registry

**File:** `src/mcp/server.rs` (modify existing)

```rust
ToolDefinition {
    name: "repo/clone".to_string(),
    description: Some(
        "Clone a Git repository and stream its contents. \
         Returns a base64-encoded tar.gz archive that you should \
         extract to your working directory. After extraction, run \
         'git init' to initialize a fresh git repository. \
         NOTE: Files are streamed directly from the remote — they \
         are NOT stored on the user's machine.".to_string()
    ),
    input_schema: json!({
        "type": "object",
        "properties": {
            "url": {
                "type": "string",
                "description": "Repository URL (HTTPS or SSH)"
            },
            "branch": {
                "type": "string",
                "description": "Branch to clone (default: main)"
            },
            "depth": {
                "type": "integer",
                "description": "Shallow clone depth (default: full)"
            },
            "sparse": {
                "type": "array",
                "items": {"type": "string"},
                "description": "Sparse checkout paths (default: all)"
            }
        },
        "required": ["url"]
    }),
}
```

**Acceptance criteria:**

- [ ] `repo/clone` tool registered and visible in `tools/list`
- [ ] Clone public repo works (no auth needed)
- [ ] Clone private repo works with credential helper
- [ ] Clone private repo works with SSH agent
- [ ] **NO files written to user's disk** (verify with strace/fs_usage)
- [ ] Archive is valid tar.gz (AI can extract with `tar -xzf`)
- [ ] Bare repo temp directory cleaned up automatically
- [ ] No credential leakage in logs or error messages
- [ ] Large repo doesn't OOM (streaming works)

---

### 1.4 Implement Push (repo/push tool) — ZERO DISK WRITES

**Goal:** Receive commits from AI's VM and push to remote WITHOUT writing files.

#### 1.4.1 Receive and Apply Bundle (In-Memory)

**File:** `src/streaming/bundle.rs`

```rust
use git2::{Repository, Oid};
use std::io::Cursor;

/// Process a git bundle and push its contents to a remote.
/// 
/// The bundle contains commits from the AI's work. We:
/// 1. Create a minimal temp bare repo
/// 2. Unbundle into it (git objects only)
/// 3. Push to remote with credentials
/// 4. Clean up
/// 
/// NO working tree files are ever created.
pub fn process_bundle_and_push(
    bundle_data: &[u8],
    remote_url: &str,
    target_branch: &str,
    callbacks: RemoteCallbacks,
) -> Result<PushResult, Error> {
    // Create temp bare repo to receive bundle
    let temp_dir = tempfile::tempdir()?;
    let repo = Repository::init_bare(temp_dir.path())?;
    
    // Write bundle to temp file (this is just the bundle, not repo files)
    let bundle_path = temp_dir.path().join("input.bundle");
    std::fs::write(&bundle_path, bundle_data)?;
    
    // Unbundle - this adds objects to our bare repo
    // No working tree checkout happens
    let mut remote = repo.remote_anonymous(bundle_path.to_str().unwrap())?;
    remote.fetch(&["refs/heads/*:refs/heads/*"], None, None)?;
    
    // Now push to actual remote with auth
    let mut real_remote = repo.remote_anonymous(remote_url)?;
    let mut push_opts = git2::PushOptions::new();
    push_opts.remote_callbacks(callbacks);
    
    let refspec = format!("refs/heads/{target_branch}");
    real_remote.push(&[&refspec], Some(&mut push_opts))?;
    
    // Get the pushed commit SHA
    let reference = repo.find_reference(&format!("refs/heads/{target_branch}"))?;
    let commit = reference.peel_to_commit()?.id();
    
    // temp_dir cleaned up on drop
    Ok(PushResult {
        branch: target_branch.to_string(),
        commit: commit.to_string(),
    })
}
```

**Note:** We do write the bundle to a temp file because git2 needs a path for unbundling. But this is just the bundle itself (a git transport format), not repository source files. The temp dir is cleaned up immediately after.

#### 1.4.2 MCP Tool Handler

**File:** `src/mcp/tools/repo_push.rs`

```rust
#[derive(Deserialize)]
pub struct RepoPushArgs {
    /// Remote repository URL
    pub url: String,
    /// Branch to push to
    pub branch: String,
    /// Base64-encoded git bundle from AI's repo
    pub bundle: String,
}

#[derive(Serialize)]
pub struct RepoPushResult {
    pub success: bool,
    pub branch: String,
    pub commit: String,
    pub url: String,  // URL to view the commit
}

pub async fn handle_repo_push(args: RepoPushArgs) -> Result<RepoPushResult, ToolError> {
    // 1. Decode bundle
    let bundle_data = BASE64.decode(&args.bundle)
        .map_err(|e| ToolError::InvalidInput(format!("Invalid base64: {e}")))?;
    
    // 2. Create auth callbacks
    let callbacks = crate::git2_ops::auth::create_callbacks();
    
    // 3. Process bundle and push (minimal disk use, no source files)
    let result = crate::streaming::bundle::process_bundle_and_push(
        &bundle_data,
        &args.url,
        &args.branch,
        callbacks,
    )?;
    
    // 4. Construct result URL
    let commit_url = format_commit_url(&args.url, &result.commit);
    
    Ok(RepoPushResult {
        success: true,
        branch: result.branch,
        commit: result.commit,
        url: commit_url,
    })
}
```

**Acceptance criteria:**

- [ ] `repo/push` tool registered and visible in `tools/list`
- [ ] Can push to existing branch
- [ ] Can create and push to new branch
- [ ] Protected branch rejection works
- [ ] Force push rejection works (unless configured)
- [ ] **NO source files written to user's disk**
- [ ] Bundle temp file cleaned up
- [ ] Returns valid commit URL
- [ ] No credential leakage

---

### 1.5 Session Management

**Goal:** Track active repo sessions to enable incremental operations.

**File:** `src/session.rs`

```rust
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

#[derive(Clone)]
pub struct RepoSession {
    pub url: String,
    pub branch: String,
    pub last_commit: String,
    pub cloned_at: std::time::Instant,
    // NO file paths - we don't store files!
}

#[derive(Default)]
pub struct SessionManager {
    sessions: Arc<RwLock<HashMap<String, RepoSession>>>,
}

impl SessionManager {
    pub fn record_clone(&self, url: &str, branch: &str, commit: &str) {
        let session = RepoSession {
            url: url.to_string(),
            branch: branch.to_string(),
            last_commit: commit.to_string(),
            cloned_at: std::time::Instant::now(),
        };
        let key = format!("{url}:{branch}");
        self.sessions.write().unwrap().insert(key, session);
    }
    
    pub fn get_session(&self, url: &str, branch: &str) -> Option<RepoSession> {
        let key = format!("{url}:{branch}");
        self.sessions.read().unwrap().get(&key).cloned()
    }
    
    pub fn update_commit(&self, url: &str, branch: &str, commit: &str) {
        let key = format!("{url}:{branch}");
        if let Some(session) = self.sessions.write().unwrap().get_mut(&key) {
            session.last_commit = commit.to_string();
        }
    }
    
    pub fn clear(&self) {
        self.sessions.write().unwrap().clear();
    }
}
```

**What sessions track:** URL, branch, last commit SHA
**What sessions DON'T track:** File paths (we don't store files!)

**Acceptance criteria:**

- [ ] Sessions tracked across tool calls
- [ ] Sessions cleared on client disconnect
- [ ] Thread-safe (multiple concurrent operations)
- [ ] No file path storage
- [ ] No credential storage

---

### 1.6 Integration Testing

**File:** `tests/integration_clone_push.rs`

```rust
#[cfg(feature = "integration")]
#[tokio::test]
async fn test_clone_push_workflow() {
    let repo_url = std::env::var("GIT_TEST_REPO_URL")
        .expect("Set GIT_TEST_REPO_URL");
    
    // 1. Clone
    let clone_result = handle_repo_clone(RepoCloneArgs {
        url: repo_url.clone(),
        branch: Some("main".to_string()),
        depth: None,
        sparse: None,
    }).await.unwrap();
    
    assert!(!clone_result.archive.is_empty());
    
    // 2. Verify NO files on disk (key test!)
    // The only temp files should be in system temp dir and cleaned up
    
    // 3. Verify archive is valid
    let archive_data = BASE64.decode(&clone_result.archive).unwrap();
    let tar = flate2::read::GzDecoder::new(&archive_data[..]);
    let mut archive = tar::Archive::new(tar);
    assert!(archive.entries().unwrap().count() > 0);
}

#[cfg(feature = "integration")]
#[tokio::test]
async fn test_no_disk_writes() {
    // Use inotify/FSEvents to verify no writes outside temp
    // This is the critical security test
}
```

**Acceptance criteria:**

- [ ] Integration tests pass
- [ ] Verified: no files written to user's home/documents
- [ ] Verified: temp files cleaned up
- [ ] Works with both HTTPS and SSH auth

---

## Phase 1 Completion Checklist

Before moving to Phase 2, ALL of these must be done:

- [ ] `repo/clone` streams without disk writes
- [ ] `repo/push` works without storing source files
- [ ] Session management implemented
- [ ] Integration tests pass
- [ ] **Verified: NO repository files on user's disk at any point**
- [ ] No credential leakage (code audit)
- [ ] Documentation updated
- [ ] CHANGELOG.md updated

---

## Phase 2: Full Workflow Support

### 2.1 Incremental Sync (repo/pull tool)

**Goal:** Fetch only new commits since last clone/pull, stream delta.

```rust
#[derive(Deserialize)]
pub struct RepoPullArgs {
    pub url: String,
    pub branch: String,
    pub since_commit: Option<String>,  // What AI currently has
}
```

**Implementation:**
1. Fetch to bare repo
2. Find commits between `since_commit` and new HEAD
3. Walk trees, find changed files
4. Stream only changed file contents as tar
5. **NO full checkout, NO disk writes**

### 2.2 Shallow Clone Support

- Add `depth` parameter to fetch
- Reduces objects fetched
- Faster for large repos with long history

### 2.3 Sparse Checkout Support

- Filter tree walk to only specified paths
- `sparse: ["src/", "Cargo.toml"]`
- Reduces archive size dramatically

---

## Phase 3: Production Hardening

### 3.1 Chunked Transfer

- For repos > 50MB
- Stream in chunks
- AI reassembles

### 3.2 Memory Limits

- Don't load entire archive into memory
- Stream directly to base64 encoder
- Backpressure handling

### 3.3 Error Handling

| Error | Message (no secrets!) |
|-------|----------------------|
| Auth failed | "Authentication failed. Check credential helper config." |
| Repo not found | "Repository not found: {url}" |
| Network timeout | "Network timeout. Please try again." |
| Invalid bundle | "Invalid git bundle format." |

---

## Phase 4: Provider Support

- GitHub (HTTPS, SSH)
- GitLab (HTTPS, SSH, self-hosted)
- Bitbucket (if needed)

---

## Phase 5: Advanced Features

- Branch operations (create, delete, list)
- Submodule support
- LFS support

---

## Migration from v1

### Files to Remove

```
src/git/command.rs    # CLI parsing - not needed
src/git/executor.rs   # CLI execution - replaced by git2
```

### Files to Keep

```
src/git/sanitiser.rs  # May still be useful
src/security/*        # Guards still apply
src/mcp/protocol.rs   # Unchanged
src/mcp/transport.rs  # Unchanged
src/config/*          # Extend for new options
```

---

## Success Metrics

| Metric | Target |
|--------|--------|
| Clone 100 files | < 5 seconds |
| Push 10 commits | < 5 seconds |
| User disk writes | **ZERO** (verified) |
| Memory usage | < 100 MB for typical repos |
| vs GitHub MCP | 10x+ faster |

---

## References

- [git2 crate docs](https://docs.rs/git2/latest/git2/)
- [git2 examples](https://github.com/rust-lang/git2-rs/tree/master/examples)
- [libgit2 authentication](https://libgit2.org/docs/guides/authentication/)
- [Git bundle format](https://git-scm.com/docs/git-bundle)

---

*Last updated: 2026-01-10*
