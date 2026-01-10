# git-proxy-mcp — AI Assistant Context

## The Vision (Read First!)

See `docs/VISION.md` for the full three-tier architecture:

| Tier | Data Flow | Status |
|------|-----------|--------|
| **Tier 1** | GitHub → MCP (RAM) → AI | Current focus |
| **Tier 2** | GitHub → MCP (chunks) → AI | Future |
| **Tier 3** | GitHub ↔ AI directly! ⭐ | Ultimate goal |

**Tier 3 is THE GOLDEN GOAL:** User's PC becomes a pure authentication broker. It never sees the code. AI connects directly to GitHub using short-lived tokens.

## What Is This Project?

A secure credential proxy for cloud-based AI assistants (Claude.ai, ChatGPT, Gemini) to work with private Git repositories.

**Current implementation (Tier 1):** Stream Git data through memory on user's PC.  
**Ultimate goal (Tier 3):** AI connects directly to GitHub; user's PC only provides tokens.

## Quick Reference

| Resource | Location |
|----------|----------|
| Vision (3 tiers) | `docs/VISION.md` |
| Battle plan | `TODO.md` |
| Architecture | `docs/ARCHITECTURE.md` |

## Critical Rules

### 🔴 NEVER Do These

1. **NEVER store credentials**
2. **NEVER log credentials**
3. **NEVER checkout working tree** (use bare repos)
4. **NEVER write source files to disk**
5. **NEVER push to main**

### 🟢 ALWAYS Do These

1. **Use `Repository::init_bare()`**
2. **Use `repo.find_blob().content()`** (read from object DB)
3. **Use `tar::Builder::new(Vec::new())`** (build in memory)
4. **Use git2 credential callbacks**
5. **Use `TempDir`** (auto-cleanup)

## Tier 1 Implementation Pattern

```rust
// CORRECT: Bare repo, walk tree, stream blobs
pub fn create_tar_from_tree(repo: &Repository, commit_id: Oid) -> Vec<u8> {
    let tree = repo.find_commit(commit_id)?.tree()?;
    
    let mut buffer = Vec::new();
    let encoder = GzEncoder::new(&mut buffer, Compression::fast());
    let mut tar = tar::Builder::new(encoder);
    
    tree.walk(TreeWalkMode::PreOrder, |dir, entry| {
        if entry.kind() == Some(ObjectType::Blob) {
            let blob = repo.find_blob(entry.id())?;
            tar.append_data(&mut header, path, blob.content())?;
        }
        TreeWalkResult::Ok
    })?;
    
    buffer
}
```

## Tier 3 Preview (The Goal)

```rust
// GOLDEN: Generate token, AI clones directly
pub async fn handle_get_token(url: &str) -> TokenResponse {
    let app = GitHubApp::load()?;
    let token = app.create_installation_token(url, Duration::hours(1))?;
    
    TokenResponse {
        token: token.value,
        clone_url: format!("https://x-access-token:{token}@github.com/..."),
        expires_at: token.expires_at,
    }
}

// AI then runs: git clone https://x-access-token:TOKEN@github.com/...
// ZERO bytes through user's PC!
```

## Project Structure

```
src/
├── git2_ops/           # git2 library operations
│   ├── auth.rs         # Credential callbacks
│   ├── clone.rs        # Bare fetch + tree streaming
│   └── push.rs         # Bundle processing
├── streaming/          # Transfer formats
│   ├── tar.rs          # Tree → tar.gz (in memory)
│   └── bundle.rs       # Git bundle handling
├── mcp/tools/          # MCP tool handlers
│   ├── repo_clone.rs   # Tier 1: stream tar
│   ├── repo_push.rs    # Tier 1: receive bundle
│   └── auth_token.rs   # Tier 3: generate token
├── session.rs          # Session tracking
└── security/           # Guards from v1
```

## Current Phase

**Phase 1: Tier 1 Foundation** ← WE ARE HERE

See `TODO.md` for detailed steps.

## Off Limits

**`CODE_OF_CONDUCT.md`** — Do not modify.
