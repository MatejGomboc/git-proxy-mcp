# git-proxy-mcp — AI Assistant Context

## The Vision (Read First!)

See `docs/VISION.md` for the full architecture:

| Tier | Data Flow |
|------|-----------|
| **Tier 1** | GitHub → MCP (RAM) → AI |
| **Tier 2** | GitHub → MCP (chunks) → AI |

**Core principle:** Credentials NEVER leave user's PC. Files stream through MCP to AI's VM.

## What Is This Project?

A secure credential relay for cloud-based AI assistants (Claude.ai, ChatGPT, Gemini) to work with private Git repositories.

**Tier 1:** Stream Git data through memory on user's PC.
**Tier 2:** Chunked streaming for large repos (production-ready).

## Quick Reference

| Resource | Location |
|----------|----------|
| Vision | `docs/VISION.md` |
| Architecture | `docs/ARCHITECTURE.md` |

## Critical Rules

### NEVER Do These

1. **NEVER store credentials**
2. **NEVER log credentials**
3. **NEVER checkout working tree** (use bare repos)
4. **NEVER write source files to disk**
5. **NEVER push to main**
6. **NEVER send credentials to AI** (not even short-lived tokens)

### ALWAYS Do These

1. **Use `Repository::init_bare()`**
2. **Use `repo.find_blob().content()`** (read from object DB)
3. **Use `tar::Builder::new(Vec::new())`** (build in memory)
4. **Use git2 credential callbacks**
5. **Use `TempDir`** (auto-cleanup)

## Implementation Pattern

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
│   ├── repo_clone.rs   # Stream tar to AI
│   └── repo_push.rs    # Receive bundle from AI
├── session.rs          # Session tracking
└── security/           # Security guards
```

## Off Limits

**`CODE_OF_CONDUCT.md`** — Do not modify.
