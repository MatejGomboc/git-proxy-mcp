# TODO — Development Battle Plan

## The Vision: Three Tiers

See `docs/VISION.md` for the full architectural vision. Summary:

| Tier | Description | Data Flow |
|------|-------------|----------|
| **Tier 1** | Memory buffer | GitHub → MCP (RAM) → AI |
| **Tier 2** | Chunked streaming | GitHub → MCP (chunks) → AI |
| **Tier 3** | Token delegation ⭐ | GitHub ↔ AI directly! |

**Current focus:** Tier 1 (get it working), then evolve toward Tier 3.

---

## Overview

**Goal:** Build a secure credential proxy that enables cloud-based AI assistants to work with private Git repositories.

**Target Users:**
- Claude.ai (with computer use)
- ChatGPT with Code Interpreter  
- Gemini with code execution
- Any sandboxed AI environment

**Non-Targets:** Local AI tools (Claude Code, Cursor, Aider) — already have direct Git access.

---

## Architecture: Tier 1 (First Draft)

```
GitHub                     User's PC                    AI's VM
   │                          │                           │
   │  git2 fetch (bare)       │                           │
   ├─────────────────────────►│                           │
   │                          │                           │
   │                     Stream blobs to                  │
   │                     tar (in memory)                  │
   │                          │                           │
   │                          │  MCP response             │
   │                          ├─────────────────────────►│
   │                          │  (base64 tar.gz)          │
   │                          │                           │
   │                     NO FILES on disk                 │
   │                     (objects in temp bare repo)      │
```

**Key constraint:** No source files written to user's disk. We use bare repositories and stream blob contents directly from git's object database to an in-memory tar archive.

---

## Phase 1: Foundation (Tier 1) ← CURRENT

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

**Cargo.toml:**

```toml
[dependencies]
git2 = "0.19"
flate2 = "1.0"
tar = "0.4"
base64 = "0.21"
tempfile = "3.10"
```

**Acceptance criteria:**
- [ ] `cargo build` succeeds
- [ ] Module structure in place
- [ ] Smoke test: can create `git2::Repository`

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
- [ ] Compiles
- [ ] SSH agent path implemented
- [ ] Credential helper path implemented
- [ ] Code audit: no credential leakage

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
- [ ] `repo/clone` tool works
- [ ] Public repos clone without auth
- [ ] Private repos clone with credential helper
- [ ] Private repos clone with SSH agent
- [ ] **NO source files on disk** (verify!)
- [ ] Valid tar.gz output
- [ ] Temp bare repo cleaned up

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
- [ ] `repo/push` tool works
- [ ] Push to existing branch
- [ ] Create and push to new branch
- [ ] Protected branch guard works
- [ ] Bundle temp file cleaned up

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
# Verify no disk writes
strace -f -e write cargo run 2>&1 | grep -v /tmp
```

---

## Phase 2: Tier 1 Hardening

- [ ] Error handling matrix
- [ ] Shallow clone support
- [ ] Sparse checkout support
- [ ] Audit logging

---

## Phase 3: Tier 2 (Chunked Streaming)

- [ ] Stream tar in chunks instead of buffering
- [ ] Handle repos > available RAM
- [ ] Resume interrupted transfers

---

## Phase 4: Tier 3 (Token Delegation) ⭐ THE GOAL

- [ ] Create GitHub App
- [ ] Implement `auth/get_token` tool
- [ ] AI clones directly from GitHub
- [ ] **ZERO bytes through user's PC!**

```rust
#[derive(Serialize)]
pub struct TokenResponse {
    pub token: String,           // ghs_xxxx
    pub expires_at: DateTime,    // 1 hour from now
    pub clone_url: String,       // https://x-access-token:TOKEN@github.com/...
    pub permissions: Vec<String>,
}

pub async fn handle_get_token(args: GetTokenArgs) -> Result<TokenResponse, ToolError> {
    let app = GitHubApp::load()?;
    let installation = app.find_installation(&args.url)?;
    
    let token = installation.create_access_token(
        repos: [&args.url],
        permissions: { contents: "write" },
        expires: Duration::hours(1),
    ).await?;
    
    Ok(TokenResponse {
        token: token.value,
        expires_at: token.expires_at,
        clone_url: format!("https://x-access-token:{}@github.com/{}", 
                          token.value, args.repo_path),
        permissions: token.permissions,
    })
}
```

---

## Phase 5: Multi-Provider

- [ ] GitLab (Project Access Tokens)
- [ ] Bitbucket (Repository Access Tokens)
- [ ] Azure DevOps

---

## Success Metrics

| Metric | Tier 1 | Tier 2 | Tier 3 |
|--------|--------|--------|--------|
| Files on user's disk | None | None | None |
| Data through user's PC | Yes (RAM) | Yes (chunked) | **NONE** |
| Clone 100 files | < 5s | < 5s | < 3s |
| Memory for large repo | O(repo) | O(chunk) | O(1) |

---

## References

- [git2 crate](https://docs.rs/git2/latest/git2/)
- [GitHub Apps](https://docs.github.com/en/apps)
- [Installation Access Tokens](https://docs.github.com/en/apps/creating-github-apps/authenticating-with-a-github-app/generating-an-installation-access-token-for-a-github-app)

---

*Tier 1 gets us working. Tier 3 gets us perfect.*
