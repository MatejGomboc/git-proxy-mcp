# TODO — Development Battle Plan

## The Vision: Credential Relay

See `docs/VISION.md` for the full architectural vision. Summary:

| Tier | Description | Data Flow |
|------|-------------|----------|
| **Tier 1** | Memory buffer | GitHub → MCP (RAM) → AI |
| **Tier 2** | Chunked streaming | GitHub → MCP (chunks) → AI |

**Current focus:** Tier 1 (get it working), then Tier 2 (production-ready).

---

## Overview

**Goal:** Build a secure credential relay that enables cloud-based AI assistants to work with private Git repositories. Credentials never leave the user's PC.

**Target Users:**

- Claude.ai (with computer use)
- ChatGPT with Code Interpreter
- Gemini with code execution
- Any sandboxed AI environment

**Non-Targets:** Local AI tools (Claude Code, Cursor, Aider) — they already have direct Git access.

---

## Architecture: Credential Relay

```
GitHub                     User's PC                    AI's VM
   │                          │                           │
   │◄──── credentials ────────┤                           │
   │      (SSH/PAT)           │                           │
   │                          │                           │
   │  git2 fetch (bare)       │                           │
   ├─────────────────────────►│                           │
   │                          │                           │
   │                     Stream blobs to                  │
   │                     tar (in memory)                  │
   │                          │                           │
   │                          │  MCP response             │
   │                          ├─────────────────────────►│
   │                          │  (file contents only)     │
   │                          │                           │
   │                     CREDENTIALS stay here            │
   │                     FILES flow to AI                 │
```

**Key constraint:** No credentials leave user's PC. No source files written to user's disk.

---

## Phase 1: Foundation (Tier 1) ✅ COMPLETE

### 1.1 Add git2 Dependency and Module Structure

**Goal:** Set up git2 and create the module skeleton.

**Files to create/modify:**

```
Cargo.toml                 # Add dependencies
src/
├── git2_ops/              # git2 operations
│   ├── mod.rs
│   ├── auth.rs            # Credential callbacks
│   ├── clone.rs           # Bare fetch + tree streaming
│   ├── push.rs            # Bundle processing + push
│   └── error.rs
└── streaming/             # In-memory transfer handling
    ├── mod.rs
    ├── tar.rs             # Tree → tar.gz (in memory)
    └── bundle.rs          # Git bundle handling
```

**Cargo.toml additions:**

```toml
[dependencies]
git2 = "0.19"
flate2 = "1.0"
tar = "0.4"
base64 = "0.21"
tempfile = "3.10"
```

**Acceptance criteria:**

- [x] `cargo build` succeeds
- [x] Module structure in place
- [x] Smoke test: can create `git2::Repository`

---

### 1.2 Implement Credential Callbacks

**File:** `src/git2_ops/auth.rs`

```rust
use git2::{Cred, CredentialType, RemoteCallbacks};

pub fn create_callbacks() -> RemoteCallbacks<'static> {
    let mut callbacks = RemoteCallbacks::new();

    callbacks.credentials(|url, username_from_url, allowed_types| {
        // SSH agent (key never leaves agent)
        if allowed_types.contains(CredentialType::SSH_KEY) {
            if let Some(username) = username_from_url {
                return Cred::ssh_key_from_agent(username);
            }
        }

        // Credential helper (system keychain)
        if allowed_types.contains(CredentialType::USER_PASS_PLAINTEXT) {
            let config = git2::Config::open_default()?;
            return Cred::credential_helper(&config, url, username_from_url);
        }

        Err(git2::Error::from_str("no suitable credential method"))
    });

    callbacks
}
```

**Security rules:**

- NEVER log `Cred` objects
- NEVER store credentials
- NEVER include credentials in error messages

**Acceptance criteria:**

- [x] Compiles
- [x] SSH agent path implemented
- [x] Credential helper path implemented
- [x] Code audit: no credential leakage

---

### 1.3 Implement Streaming Clone

**Goal:** Fetch repo and stream contents as tar.gz WITHOUT writing source files to disk.

#### 1.3.1 Bare Fetch

**File:** `src/git2_ops/clone.rs`

```rust
use git2::{Repository, FetchOptions, Oid};
use tempfile::TempDir;

pub struct FetchResult {
    pub repo: Repository,
    pub head_commit: Oid,
    pub branch: String,
    pub _temp_dir: TempDir,  // Prevent cleanup until we're done
}

pub fn fetch_repo(
    url: &str,
    branch: Option<&str>,
    callbacks: RemoteCallbacks,
) -> Result<FetchResult, git2::Error> {
    // Create BARE repo - no working tree!
    let temp_dir = TempDir::new()?;
    let repo = Repository::init_bare(temp_dir.path())?;

    // Add remote and fetch
    let mut remote = repo.remote_anonymous(url)?;
    let mut fetch_opts = FetchOptions::new();
    fetch_opts.remote_callbacks(callbacks);

    let branch_name = branch.unwrap_or("main");
    let refspec = format!("refs/heads/{branch_name}:refs/heads/{branch_name}");

    remote.fetch(&[&refspec], Some(&mut fetch_opts), None)?;

    let reference = repo.find_reference(&format!("refs/heads/{branch_name}"))?;
    let head_commit = reference.peel_to_commit()?.id();

    Ok(FetchResult {
        repo,
        head_commit,
        branch: branch_name.to_string(),
        _temp_dir: temp_dir,
    })
}
```

**Key:** `Repository::init_bare()` — NO working tree, NO source files!

#### 1.3.2 Stream Tree to Tar

**File:** `src/streaming/tar.rs`

```rust
use flate2::write::GzEncoder;
use flate2::Compression;
use git2::{Repository, Oid, ObjectType, TreeWalkMode, TreeWalkResult};
use std::io::Write;

pub fn create_tar_from_tree(
    repo: &Repository,
    commit_id: Oid,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let commit = repo.find_commit(commit_id)?;
    let tree = commit.tree()?;

    let mut archive_buffer = Vec::new();
    {
        let encoder = GzEncoder::new(&mut archive_buffer, Compression::fast());
        let mut tar = tar::Builder::new(encoder);

        tree.walk(TreeWalkMode::PreOrder, |dir, entry| {
            let name = match entry.name() {
                Some(n) => n,
                None => return TreeWalkResult::Skip,
            };

            let path = if dir.is_empty() {
                name.to_string()
            } else {
                format!("{dir}{name}")
            };

            if entry.kind() == Some(ObjectType::Blob) {
                if let Ok(blob) = repo.find_blob(entry.id()) {
                    let content = blob.content();

                    let mut header = tar::Header::new_gnu();
                    let _ = header.set_path(&path);
                    header.set_size(content.len() as u64);
                    header.set_mode(entry.filemode() as u32);
                    header.set_cksum();

                    let _ = tar.append(&header, content);
                }
            }

            TreeWalkResult::Ok
        })?;

        tar.finish()?;
    }

    Ok(archive_buffer)
}
```

**Key insight:**

- `repo.find_blob(id)` reads from git object database
- `blob.content()` gives raw bytes — never written to disk
- `tar::Builder::new(Vec::new())` builds archive in memory

#### 1.3.3 MCP Tool Handler

**File:** `src/mcp/tools/repo_clone.rs`

```rust
#[derive(Deserialize)]
pub struct RepoCloneArgs {
    pub url: String,
    pub branch: Option<String>,
    pub depth: Option<u32>,
    pub sparse: Option<Vec<String>>,
}

#[derive(Serialize)]
pub struct RepoCloneResult {
    pub archive: String,  // Base64 tar.gz
    pub commit: String,
    pub branch: String,
    pub file_count: usize,
    pub archive_size: usize,
}

pub async fn handle_repo_clone(args: RepoCloneArgs) -> Result<RepoCloneResult, ToolError> {
    let callbacks = crate::git2_ops::auth::create_callbacks();

    let fetch_result = crate::git2_ops::clone::fetch_repo(
        &args.url,
        args.branch.as_deref(),
        callbacks,
    )?;

    let archive_data = crate::streaming::tar::create_tar_from_tree(
        &fetch_result.repo,
        fetch_result.head_commit,
    )?;

    let archive_base64 = base64::engine::general_purpose::STANDARD.encode(&archive_data);

    Ok(RepoCloneResult {
        archive: archive_base64,
        commit: fetch_result.head_commit.to_string(),
        branch: fetch_result.branch,
        file_count: count_files(&archive_data)?,
        archive_size: archive_data.len(),
    })
}
```

**Acceptance criteria:**

- [x] `repo/clone` tool works
- [x] Public repos clone without auth
- [x] Private repos clone with credential helper
- [x] Private repos clone with SSH agent
- [x] **NO source files on disk** (verify!)
- [x] Valid tar.gz output
- [x] Temp bare repo cleaned up

---

### 1.4 Implement Push

**File:** `src/streaming/bundle.rs`

```rust
pub fn process_bundle_and_push(
    bundle_data: &[u8],
    remote_url: &str,
    target_branch: &str,
    callbacks: RemoteCallbacks,
) -> Result<PushResult, Error> {
    let temp_dir = TempDir::new()?;
    let repo = Repository::init_bare(temp_dir.path())?;

    // Write bundle to temp (just the bundle, not source files)
    let bundle_path = temp_dir.path().join("input.bundle");
    std::fs::write(&bundle_path, bundle_data)?;

    // Unbundle (adds objects to bare repo)
    let mut remote = repo.remote_anonymous(bundle_path.to_str().unwrap())?;
    remote.fetch(&["refs/heads/*:refs/heads/*"], None, None)?;

    // Push to real remote with auth
    let mut real_remote = repo.remote_anonymous(remote_url)?;
    let mut push_opts = git2::PushOptions::new();
    push_opts.remote_callbacks(callbacks);

    real_remote.push(
        &[&format!("refs/heads/{target_branch}")],
        Some(&mut push_opts),
    )?;

    let reference = repo.find_reference(&format!("refs/heads/{target_branch}"))?;
    let commit = reference.peel_to_commit()?.id();

    Ok(PushResult {
        branch: target_branch.to_string(),
        commit: commit.to_string(),
    })
}
```

**Acceptance criteria:**

- [x] `repo/push` tool works
- [x] Push to existing branch
- [x] Create and push to new branch
- [x] Protected branch guard works
- [x] Bundle temp file cleaned up

---

### 1.5 Session Management

**File:** `src/session.rs`

```rust
pub struct RepoSession {
    pub url: String,
    pub branch: String,
    pub last_commit: String,
    // NO file paths - we don't store files!
}

pub struct SessionManager {
    sessions: Arc<RwLock<HashMap<String, RepoSession>>>,
}
```

---

### 1.6 Integration Testing

```bash
# Verify no disk writes (Linux)
strace -f -e write cargo run 2>&1 | grep -v /tmp
```

---

## Phase 2: Tier 1 Hardening ✅ COMPLETE

- [x] Comprehensive error handling
- [x] Shallow clone support (`depth` parameter)
- [x] Sparse checkout support (`sparse` parameter)
- [x] Audit logging for all operations
- [x] Rate limiting integration
- [x] Security guards (branch protection, force push)

---

## Phase 3: Tier 2 (Chunked Streaming) ✅ COMPLETE

- [x] Stream tar in chunks instead of buffering entire repo
- [x] Handle repos larger than available RAM (via multi-call protocol)
- [x] Resume interrupted transfers (chunks can be requested in any order)
- [x] Progress reporting (total_chunks, chunk_index, is_last)

**New tools:**

- `repo/clone_start` — Start chunked clone, returns session_id and total_chunks
- `repo/clone_chunk` — Get chunk by index (base64 encoded)
- `repo/clone_cancel` — Cancel session and free resources

**Tier 2 is now production-ready!**

---

## Phase 4: Polish & Release <- CURRENT

- [x] Multi-provider support (GitLab, Bitbucket, Azure DevOps, self-hosted)
- [x] Comprehensive documentation (README, ARCHITECTURE, rustdoc)
- [ ] Performance benchmarks
- [ ] Security audit

---

## Success Metrics

| Metric | Tier 1 ✅ | Tier 2 ✅ |
|--------|--------|--------|
| Files on user's disk | None | None |
| Credentials to AI | **NEVER** | **NEVER** |
| Data through user's PC | Yes (RAM) | Yes (chunked) |
| Clone 100 files | < 5s | < 5s |
| Memory for large repo | O(repo) | O(chunk) |

**Both tiers are now implemented!**

---

## References

- [git2 crate](https://docs.rs/git2/latest/git2/)
- [MCP Specification](https://modelcontextprotocol.io/)
- [libgit2 authentication](https://libgit2.org/docs/guides/authentication/)

---

*Tier 1 gets us working. Tier 2 gets us production-ready.*
