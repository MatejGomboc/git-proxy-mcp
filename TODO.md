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
We provide:         Authenticated streaming proxy
Result:             AI works on private repos like a local developer
```

---

## Architecture: Pure Credential Proxy

```
GIT PROVIDER                 YOUR PC                    AI's VM
(GitHub/GitLab)             (MCP Server)              (Claude.ai)

┌──────────────┐      ┌─────────────────┐      ┌─────────────────┐
│              │      │ git-proxy-mcp   │      │                 │
│ repo objects │◄────►│                 │◄────►│ /home/claude/   │
│              │      │ • Credentials   │      │   repo/         │
│              │      │ • git2 library  │      │   .git/         │
│              │      │ • NO storage    │      │   (full repo)   │
└──────────────┘      └─────────────────┘      └─────────────────┘
                             │                        │
                      Credentials stay          Files live
                      on YOUR machine           in AI's VM
```

### Data Flow

| Operation | Flow |
|-----------|------|
| Clone | GitHub → (git2 auth) → MCP streams tar.gz → AI's VM extracts |
| Push | AI's VM → git bundle/patches → MCP → (git2 auth) → GitHub |
| Pull | GitHub → (git2 auth) → MCP streams delta → AI's VM applies |

---

## Design Decisions (v2)

| Decision | Choice | Rationale |
|----------|--------|----------|
| Git library | `git2` (libgit2) | In-process, streaming capable, credential callbacks |
| File storage on MCP | **None** | Pure proxy — stream through only |
| Clone transfer format | tar.gz stream | Simple, preserves permissions, single blob |
| Push transfer format | git bundle | Native git format, preserves commits/history |
| Session state | Stateful | Track repos to enable incremental sync |
| Credential handling | System helpers via git2 | Use existing git config, never store |
| Large repo strategy | Shallow + sparse | Configurable depth and path filters |

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
│   ├── clone.rs           # Clone and streaming
│   ├── push.rs            # Receive and push
│   └── error.rs           # git2-specific errors
└── streaming/             # NEW: transfer format handling
    ├── mod.rs
    ├── tar.rs             # Tar archive creation
    └── bundle.rs          # Git bundle handling
```

**Cargo.toml additions:**

```toml
[dependencies]
git2 = "0.19"              # libgit2 bindings
flate2 = "1.0"             # gzip compression
tar = "0.4"                # tar archive creation
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

**Testing strategy:**

```rust
#[cfg(test)]
mod tests {
    // Unit tests use a mock - can't test real auth without credentials
    
    #[test]
    fn callbacks_created_successfully() {
        let callbacks = create_callbacks();
        // Just verify it doesn't panic
    }
}

// Integration test (requires real repo access)
#[cfg(feature = "integration")]
#[test]
fn test_auth_against_github() {
    // Set up in CI with test credentials
}
```

**Acceptance criteria:**

- [ ] `create_callbacks()` function exists and compiles
- [ ] SSH agent authentication path implemented
- [ ] Credential helper path implemented
- [ ] No credentials in any log output (audit the code)
- [ ] Error messages don't leak credential details

---

### 1.3 Implement Streaming Clone (repo/clone tool)

**Goal:** Clone a remote repo and stream its contents to the AI as a tar.gz archive.

**This is the most complex piece — break it down:**

#### 1.3.1 Remote Fetch (in-memory)

**File:** `src/git2_ops/clone.rs`

```rust
use git2::{Repository, FetchOptions, build::RepoBuilder};
use std::path::Path;
use tempfile::TempDir;

pub struct CloneOptions {
    pub url: String,
    pub branch: Option<String>,
    pub depth: Option<u32>,          // Shallow clone
    pub sparse_paths: Option<Vec<String>>,  // Only these paths
}

pub struct CloneResult {
    pub repo: Repository,
    pub temp_dir: TempDir,  // Temporary - deleted after streaming
    pub head_commit: git2::Oid,
}

/// Clone a repository. Files are temporary — will be streamed then deleted.
pub fn clone_repo(options: &CloneOptions, callbacks: RemoteCallbacks) -> Result<CloneResult, Error> {
    let temp_dir = TempDir::new()?;
    
    let mut fetch_opts = FetchOptions::new();
    fetch_opts.remote_callbacks(callbacks);
    
    // Shallow clone if requested
    if let Some(depth) = options.depth {
        fetch_opts.depth(depth as i32);
    }
    
    let mut builder = RepoBuilder::new();
    builder.fetch_options(fetch_opts);
    
    if let Some(ref branch) = options.branch {
        builder.branch(branch);
    }
    
    let repo = builder.clone(&options.url, temp_dir.path())?;
    let head_commit = repo.head()?.peel_to_commit()?.id();
    
    Ok(CloneResult { repo, temp_dir, head_commit })
}
```

**Note:** We DO write to a temp directory because git2 needs a working repository to walk the tree. The key is:
1. It's temporary (auto-deleted)
2. We stream out immediately
3. User never sees these files

#### 1.3.2 Create Tar Archive from Working Tree

**File:** `src/streaming/tar.rs`

```rust
use flate2::write::GzEncoder;
use flate2::Compression;
use std::io::Write;
use std::path::Path;
use tar::Builder;

/// Create a tar.gz archive of a directory, returning bytes.
pub fn create_tar_gz(source_dir: &Path, exclude_git: bool) -> Result<Vec<u8>, Error> {
    let mut archive_data = Vec::new();
    
    {
        let encoder = GzEncoder::new(&mut archive_data, Compression::fast());
        let mut builder = Builder::new(encoder);
        
        // Walk directory and add files
        for entry in walkdir::WalkDir::new(source_dir) {
            let entry = entry?;
            let path = entry.path();
            
            // Skip .git directory if requested (AI will git init fresh)
            if exclude_git && path.components().any(|c| c.as_os_str() == ".git") {
                continue;
            }
            
            let relative_path = path.strip_prefix(source_dir)?;
            
            if entry.file_type().is_file() {
                builder.append_path_with_name(path, relative_path)?;
            } else if entry.file_type().is_dir() {
                builder.append_dir(relative_path, path)?;
            }
        }
        
        builder.finish()?;
    }
    
    Ok(archive_data)
}
```

**Add to Cargo.toml:**

```toml
walkdir = "2.4"
```

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
    /// Base64-encoded tar.gz archive
    pub archive: String,
    /// Commit SHA that was cloned
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
    
    // 2. Clone to temp directory
    let clone_result = crate::git2_ops::clone::clone_repo(
        &CloneOptions {
            url: args.url.clone(),
            branch: args.branch.clone(),
            depth: args.depth,
            sparse_paths: args.sparse.clone(),
        },
        callbacks,
    )?;
    
    // 3. Create tar.gz archive
    let archive_data = crate::streaming::tar::create_tar_gz(
        clone_result.temp_dir.path(),
        true,  // exclude .git - AI will init fresh
    )?;
    
    // 4. Encode as base64 for JSON transport
    let archive_base64 = BASE64.encode(&archive_data);
    
    // 5. Clean up temp dir (automatic via TempDir drop)
    // clone_result.temp_dir is dropped here
    
    Ok(RepoCloneResult {
        archive: archive_base64,
        commit: clone_result.head_commit.to_string(),
        branch: args.branch.unwrap_or_else(|| "main".to_string()),
        file_count: count_files_in_archive(&archive_data)?,
        archive_size: archive_data.len(),
    })
}
```

**Add to Cargo.toml:**

```toml
base64 = "0.21"
```

#### 1.3.4 Update MCP Tool Registry

**File:** `src/mcp/server.rs` (modify existing)

Add the new tool to `get_tool_definitions()`:

```rust
ToolDefinition {
    name: "repo/clone".to_string(),
    description: Some(
        "Clone a Git repository and stream its contents. \
         Returns a base64-encoded tar.gz archive that you should \
         extract to your working directory. After extraction, run \
         'git init' to initialize a fresh git repository.".to_string()
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
                "description": "Branch to clone (default: default branch)"
            },
            "depth": {
                "type": "integer",
                "description": "Shallow clone depth (default: full history)"
            },
            "sparse": {
                "type": "array",
                "items": {"type": "string"},
                "description": "Sparse checkout paths (default: all files)"
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
- [ ] Shallow clone (`depth: 1`) works
- [ ] Archive is valid tar.gz (can extract with `tar -xzf`)
- [ ] Temp directory is cleaned up after streaming
- [ ] No credential leakage in logs or error messages
- [ ] Large repo (>100MB) doesn't OOM (streaming works)

---

### 1.4 Implement Push (repo/push tool)

**Goal:** Receive commits from AI's VM and push to remote.

**This is the reverse flow:**

```
AI's VM: git bundle create changes.bundle origin/main..HEAD
    ↓
MCP receives bundle (base64 in JSON)
    ↓  
git2: unbundle → apply → push
    ↓
GitHub receives commits
```

#### 1.4.1 Git Bundle Handling

**File:** `src/streaming/bundle.rs`

```rust
use std::process::Command;
use tempfile::NamedTempFile;
use std::io::Write;

/// Apply a git bundle to a repository and push.
/// 
/// The bundle should contain commits from the AI's work.
/// We unbundle to a temp repo, then push with credentials.
pub fn apply_bundle_and_push(
    bundle_data: &[u8],
    remote_url: &str,
    target_branch: &str,
    callbacks: RemoteCallbacks,
) -> Result<PushResult, Error> {
    // 1. Write bundle to temp file
    let mut bundle_file = NamedTempFile::new()?;
    bundle_file.write_all(bundle_data)?;
    
    // 2. Create temp repo to unbundle into
    let temp_dir = TempDir::new()?;
    let repo = Repository::init(temp_dir.path())?;
    
    // 3. Add bundle as remote and fetch
    let mut remote = repo.remote_anonymous(bundle_file.path().to_str().unwrap())?;
    remote.fetch(&["refs/heads/*:refs/heads/*"], None, None)?;
    
    // 4. Now push to actual remote with auth
    let mut real_remote = repo.remote_anonymous(remote_url)?;
    let mut push_opts = git2::PushOptions::new();
    push_opts.remote_callbacks(callbacks);
    
    real_remote.push(
        &[&format!("refs/heads/{target_branch}")],
        Some(&mut push_opts),
    )?;
    
    Ok(PushResult {
        branch: target_branch.to_string(),
        // Get pushed commit SHA
        commit: repo.head()?.peel_to_commit()?.id().to_string(),
    })
}
```

#### 1.4.2 MCP Tool Handler

**File:** `src/mcp/tools/repo_push.rs`

```rust
#[derive(Deserialize)]
pub struct RepoPushArgs {
    /// Remote repository URL
    pub url: String,
    /// Branch to push to
    pub branch: String,
    /// Base64-encoded git bundle
    pub bundle: String,
    /// Optional: create branch if it doesn't exist
    #[serde(default)]
    pub create_branch: bool,
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
    
    // 3. Apply and push
    let result = crate::streaming::bundle::apply_bundle_and_push(
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

fn format_commit_url(repo_url: &str, commit: &str) -> String {
    // Convert git URL to web URL
    // https://github.com/user/repo.git -> https://github.com/user/repo/commit/{sha}
    let web_url = repo_url
        .trim_end_matches(".git")
        .replace("git@github.com:", "https://github.com/");
    format!("{web_url}/commit/{commit}")
}
```

**Acceptance criteria:**

- [ ] `repo/push` tool registered and visible in `tools/list`
- [ ] Can push to existing branch
- [ ] Can create and push to new branch
- [ ] Protected branch rejection works
- [ ] Force push rejection works (unless configured)
- [ ] Returns valid commit URL
- [ ] Bundle validation (reject malformed bundles)
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
}

#[derive(Default)]
pub struct SessionManager {
    sessions: Arc<RwLock<HashMap<String, RepoSession>>>,
}

impl SessionManager {
    /// Record that a repo was cloned.
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
    
    /// Get session for a repo (for incremental pull).
    pub fn get_session(&self, url: &str, branch: &str) -> Option<RepoSession> {
        let key = format!("{url}:{branch}");
        self.sessions.read().unwrap().get(&key).cloned()
    }
    
    /// Update commit after push.
    pub fn update_commit(&self, url: &str, branch: &str, commit: &str) {
        let key = format!("{url}:{branch}");
        if let Some(session) = self.sessions.write().unwrap().get_mut(&key) {
            session.last_commit = commit.to_string();
        }
    }
    
    /// Clear all sessions (on disconnect).
    pub fn clear(&self) {
        self.sessions.write().unwrap().clear();
    }
}
```

**Usage:** Inject `SessionManager` into the MCP server and use it in tool handlers.

**Acceptance criteria:**

- [ ] Sessions tracked across tool calls
- [ ] Sessions cleared on client disconnect
- [ ] Thread-safe (multiple concurrent operations)
- [ ] No credential storage in sessions

---

### 1.6 Integration Testing

**Goal:** End-to-end tests for the complete workflow.

**File:** `tests/integration_clone_push.rs`

```rust
//! Integration tests - require network access and credentials.
//!
//! Run with: cargo test --features integration
//!
//! Requires:
//! - GIT_TEST_REPO_URL env var (a repo you have push access to)
//! - Configured credential helper or SSH agent

#[cfg(feature = "integration")]
mod integration {
    use git_proxy_mcp::*;
    
    #[tokio::test]
    async fn test_clone_push_workflow() {
        let repo_url = std::env::var("GIT_TEST_REPO_URL")
            .expect("Set GIT_TEST_REPO_URL for integration tests");
        
        // 1. Clone
        let clone_result = handle_repo_clone(RepoCloneArgs {
            url: repo_url.clone(),
            branch: Some("main".to_string()),
            depth: Some(1),
            sparse: None,
        }).await.expect("clone should succeed");
        
        assert!(!clone_result.archive.is_empty());
        assert!(!clone_result.commit.is_empty());
        
        // 2. Verify archive is valid tar.gz
        let archive_data = BASE64.decode(&clone_result.archive).unwrap();
        // ... decompress and verify ...
        
        // 3. Create a test bundle (simulating AI's work)
        // ... create bundle with test commit ...
        
        // 4. Push (to a test branch, then clean up)
        // ...
    }
}
```

**Acceptance criteria:**

- [ ] Integration test passes in CI with test credentials
- [ ] Test covers clone → push workflow
- [ ] Test cleans up (doesn't leave test branches)
- [ ] Test works with both HTTPS and SSH

---

## Phase 1 Completion Checklist

Before moving to Phase 2, ALL of these must be done:

- [ ] `repo/clone` tool works end-to-end
- [ ] `repo/push` tool works end-to-end
- [ ] Session management implemented
- [ ] Integration tests pass
- [ ] No credential leakage (code audit)
- [ ] Documentation updated
- [ ] CHANGELOG.md updated

---

## Phase 2: Full Workflow Support

### 2.1 Incremental Sync (repo/pull tool)

**Goal:** Fetch only new commits since last clone/pull.

```rust
#[derive(Deserialize)]
pub struct RepoPullArgs {
    pub url: String,
    pub branch: String,
    /// Optional: commit SHA the AI currently has
    /// If not provided, uses session state
    pub since_commit: Option<String>,
}
```

**Implementation strategy:**

1. Use session manager to find `last_commit`
2. Fetch from remote
3. Generate diff/patch between `last_commit` and new HEAD
4. Stream only changed files (not full archive)

**Acceptance criteria:**

- [ ] Detects no-op (already up to date)
- [ ] Streams only changed files
- [ ] Updates session state
- [ ] Handles force-push on remote gracefully

---

### 2.2 Shallow Clone Support

**Goal:** Support `--depth=N` for faster cloning.

Mostly already supported via git2's `FetchOptions::depth()`. Need to:

- [ ] Test shallow clone works correctly
- [ ] Document limitations (can't push some refs)
- [ ] Handle errors gracefully when shallow causes issues

---

### 2.3 Sparse Checkout Support

**Goal:** Clone only specified paths (for large repos).

```rust
pub sparse: Option<Vec<String>>,  // e.g., ["src/", "Cargo.toml"]
```

**Implementation:**

1. Clone normally (git2 doesn't support sparse clone directly)
2. When creating tar archive, filter to only sparse paths
3. This gives the AI only the files they need

**Acceptance criteria:**

- [ ] Sparse paths filter works
- [ ] Wildcard patterns supported (`src/**/*.rs`)
- [ ] Archive size reduced appropriately

---

## Phase 3: Production Hardening

### 3.1 Chunked Transfer for Large Repos

**Problem:** MCP messages have size limits. A 500MB repo won't fit.

**Solution:** Chunked streaming.

```rust
#[derive(Serialize)]
pub struct RepoCloneChunk {
    pub chunk_index: usize,
    pub total_chunks: usize,
    pub data: String,  // Base64 chunk
    pub is_last: bool,
}
```

The AI accumulates chunks and reassembles.

---

### 3.2 Error Handling Matrix

| Error | User-Facing Message | Log Level |
|-------|--------------------|-----------|
| Auth failed | "Authentication failed. Check your credential helper configuration." | WARN |
| Repo not found | "Repository not found: {url}" | INFO |
| Network timeout | "Network timeout. Please try again." | WARN |
| Invalid bundle | "Invalid git bundle format." | WARN |
| Protected branch | "Cannot push to protected branch: {branch}" | INFO |
| Rate limited | "Rate limit exceeded. Please wait." | WARN |

**NEVER include in error messages:**
- Actual credentials
- Token values
- SSH key contents
- Credential helper output

---

### 3.3 Security Audit Checklist

- [ ] `git2::Cred` objects never logged or serialised
- [ ] Temp directories have restricted permissions (0700)
- [ ] Bundle files validated before processing
- [ ] URL validation (no file:// or other dangerous schemes)
- [ ] Rate limiting prevents abuse
- [ ] Audit log captures all operations (without credentials)

---

## Phase 4: Provider Support

### 4.1 GitHub

- [ ] HTTPS + PAT via credential helper
- [ ] SSH via ssh-agent
- [ ] GitHub-specific errors (rate limits, etc.)
- [ ] Commit URL formatting

### 4.2 GitLab

- [ ] HTTPS + PAT
- [ ] SSH
- [ ] Self-hosted GitLab URL support
- [ ] Commit URL formatting

### 4.3 Bitbucket (if needed)

- [ ] HTTPS + App Password
- [ ] SSH

---

## Phase 5: Advanced Features

### 5.1 Branch Operations

```
repo/branch/create  - Create branch on remote
repo/branch/delete  - Delete branch on remote  
repo/branch/list    - List remote branches
```

### 5.2 Submodule Support

- [ ] Detect submodules in repo
- [ ] Option to include/exclude submodules
- [ ] Recursive clone support

### 5.3 LFS Support

- [ ] Detect LFS files
- [ ] Stream LFS objects separately
- [ ] Handle LFS auth

---

## Migration Path from v1

### Files to Remove (v1 implementation)

```
src/git/command.rs    # CLI command parsing - not needed
src/git/executor.rs   # CLI execution - replaced by git2
```

### Files to Keep

```
src/git/sanitiser.rs  # Still useful for output sanitisation
src/security/*        # Guards still apply
src/mcp/protocol.rs   # MCP protocol unchanged
src/mcp/transport.rs  # Transport unchanged
src/config/*          # Config still needed, extend it
```

### Files to Modify Heavily

```
src/mcp/server.rs     # New tool handlers
src/main.rs           # New initialisation
```

---

## Success Metrics

| Metric | Target | How to Measure |
|--------|--------|----------------|
| Clone 100 files | < 5 seconds | Integration test timing |
| Clone 1000 files | < 30 seconds | Integration test timing |
| Push 10 commits | < 5 seconds | Integration test timing |
| Memory usage | < 100 MB | RSS during large clone |
| vs GitHub MCP | 10x faster | Benchmark comparison |

---

## References

- [git2 crate docs](https://docs.rs/git2/latest/git2/)
- [git2 examples](https://github.com/rust-lang/git2-rs/tree/master/examples)
- [libgit2 credential handling](https://libgit2.org/docs/guides/authentication/)
- [Git bundle format](https://git-scm.com/docs/git-bundle)
- [MCP Specification](https://modelcontextprotocol.io/)

---

*Last updated: 2026-01-10*
