# AI Workflow Guide

This document explains how an AI assistant uses git-proxy-mcp to work with private repositories.

## The Complete Workflow

```text
┌─────────────────────────────────────────────────────────────────────────────┐
│                        AI's Development Workflow                            │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                             │
│  1. CLONE                                                                   │
│     AI calls: repo_clone { url: "...", branch: "main" }                    │
│     Receives: tar.gz archive                                               │
│     Extracts to: /home/claude/repo/                                        │
│     Runs: git init && git add . && git commit -m "initial"                 │
│                                                                             │
│  2. WORK (all local — no MCP calls needed)                                 │
│     git checkout -b feature/fix-bug                                        │
│     vim src/main.rs                                                        │
│     cargo test                                                             │
│     cargo fmt                                                              │
│     git add .                                                              │
│     git commit -m "Fix the bug"                                            │
│                                                                             │
│  3. PUSH                                                                    │
│     AI creates: git bundle create changes.bundle main..HEAD               │
│     AI calls: repo_push { url: "...", branch: "feature/fix-bug", bundle } │
│     Receives: { success: true, url: "https://github.com/.../commit/..." } │
│                                                                             │
│  4. ITERATE (if needed)                                                     │
│     Make more changes locally                                              │
│     Create new bundle                                                      │
│     Push again                                                             │
│                                                                             │
└─────────────────────────────────────────────────────────────────────────────┘
```

## Configuring Git Identity

When the MCP server initialises, it may provide a `gitIdentity` in the response.
This identity should be used for all commits to clearly distinguish AI-assisted
commits from human commits.

**Initialize Response (with git identity):**

```json
{
  "protocolVersion": "2024-11-05",
  "capabilities": { "tools": {} },
  "serverInfo": { "name": "git-proxy-mcp", "version": "1.1.0" },
  "gitIdentity": {
    "name": "Claude AI",
    "email": "ai-assistant@example.com"
  }
}
```

**AI configures Git before making commits:**

```bash
# Configure git identity from MCP initialise response
git config user.name "Claude AI"
git config user.email "ai-assistant@example.com"
```

This ensures all commits made by the AI are properly attributed, making it easy to:

- Filter AI-made commits in git log (`git log --author="Claude AI"`)
- Audit which changes were AI-assisted
- Maintain clear attribution in the git history

## Step-by-Step Example

### 1. Clone a Repository

**MCP Tool Call:**

```json
{
  "name": "repo_clone",
  "arguments": {
    "url": "https://github.com/user/my-rust-project",
    "branch": "main",
    "depth": 1
  }
}
```

**Response:**

```json
{
  "archive": "H4sIAAAAAAAAA+3OMQ7CMBCE... (base64)",
  "commit": "abc123def456...",
  "branch": "main",
  "file_count": 47,
  "archive_size": 1048576
}
```

**AI's Actions:**

```bash
# Decode and extract archive
echo "$ARCHIVE_BASE64" | base64 -d > repo.tar.gz
mkdir -p /home/claude/repo
tar -xzf repo.tar.gz -C /home/claude/repo
rm repo.tar.gz

# Initialise git (so we can create commits later)
cd /home/claude/repo
git init
git add .
git commit -m "Initial clone from abc123def456"

# Set up remote tracking (for reference, not for pushing)
git remote add origin https://github.com/user/my-rust-project
git branch -M main
```

### 2. Work Locally

Now the AI has a complete repository and can work normally:

```bash
# Create feature branch
git checkout -b feature/add-validation

# View the code
cat src/main.rs

# Make changes
cat > src/validation.rs << 'EOF'
pub fn validate_input(s: &str) -> bool {
    !s.is_empty() && s.len() < 1000
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_valid_input() {
        assert!(validate_input("hello"));
    }
    
    #[test]
    fn test_empty_input() {
        assert!(!validate_input(""));
    }
}
EOF

# Update main.rs to use it
echo 'mod validation;' >> src/main.rs

# Run tests
cargo test

# Format code
cargo fmt

# Check with clippy
cargo clippy

# Commit
git add .
git commit -m "Add input validation module

- Add validate_input function
- Add unit tests
- Integrate with main module"
```

### 3. Push Changes

**Create a bundle of new commits:**

```bash
# Bundle commits from main to HEAD
git bundle create /tmp/changes.bundle main..HEAD

# Convert to base64
BUNDLE_BASE64=$(base64 -w0 /tmp/changes.bundle)
```

**MCP Tool Call:**

```json
{
  "name": "repo_push",
  "arguments": {
    "url": "https://github.com/user/my-rust-project",
    "branch": "feature/add-validation",
    "bundle": "<BUNDLE_BASE64>"
  }
}
```

**Response:**

```json
{
  "success": true,
  "branch": "feature/add-validation",
  "commit": "def789...",
  "url": "https://github.com/user/my-rust-project/commit/def789..."
}
```

### 4. Pull Updates (if remote changed)

If someone else pushed changes:

**MCP Tool Call:**

```json
{
  "name": "repo_pull",
  "arguments": {
    "url": "https://github.com/user/my-rust-project",
    "branch": "main",
    "since_commit": "abc123def456..."
  }
}
```

**Response:**

```json
{
  "archive": "H4sIAAAAAAAAA+3P... (base64, only changed files)",
  "commit": "new789...",
  "changed": ["src/lib.rs", "Cargo.toml"]
}
```

**AI applies the changes:**

```bash
# Extract changed files
echo "$ARCHIVE_BASE64" | base64 -d | tar -xzf - -C /home/claude/repo

# Commit the update
cd /home/claude/repo
git add .
git commit -m "Sync from remote: new789..."
```

## Comparison: git-proxy-mcp vs GitHub MCP

### Cloning 100 Files

**GitHub MCP Server:**

```text
Call 1:  get_repository_content(path: "/")
Call 2:  get_file_content(path: "src/main.rs")
Call 3:  get_file_content(path: "src/lib.rs")
... 97 more calls ...
Call 100: get_file_content(path: "tests/integration_test.rs")

Total: 100 API calls, several minutes
```

**git-proxy-mcp:**

```text
Call 1: repo_clone { url: "..." }

Total: 1 call, seconds
```

### Running Tests

**GitHub MCP Server:**

```text
❌ Not possible — no execution environment
```

**git-proxy-mcp:**

```bash
$ cargo test
   Compiling my-project v0.1.0
    Finished test [unoptimized + debuginfo] target(s) in 2.34s
     Running unittests src/lib.rs

running 15 tests
test validation::tests::test_valid_input ... ok
test validation::tests::test_empty_input ... ok
... (all tests pass)
```

### Making Changes

**GitHub MCP Server:**

```text
Call 1: create_branch(name: "feature/fix")
Call 2: update_file(path: "src/main.rs", content: "...", message: "...")
Call 3: update_file(path: "src/lib.rs", content: "...", message: "...")
Call 4: create_pull_request(...)

Total: Many calls, each a separate commit
```

**git-proxy-mcp:**

```bash
# Local work (instant)
git checkout -b feature/fix
vim src/main.rs
vim src/lib.rs
cargo test  # Verify before committing!
git add .
git commit -m "Fix: comprehensive change with tests"

# Single push
git bundle create changes.bundle main..HEAD
# Call repo_push once

Total: 1 MCP call for all changes
```

## Tips for AI Assistants

### 1. Use Shallow Clone for Large Repos

```json
{
  "name": "repo_clone",
  "arguments": {
    "url": "https://github.com/user/huge-monorepo",
    "depth": 1,
    "sparse": ["packages/my-package/"]
  }
}
```

### 2. Bundle Multiple Commits

You can work on multiple commits locally, then push them all:

```bash
git commit -m "Add feature"
git commit -m "Add tests"
git commit -m "Update docs"

# Bundle all three
git bundle create changes.bundle main..HEAD
```

### 3. Check Before Push

Always verify locally before pushing:

```bash
cargo test
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --check
```

### 4. Handle Merge Conflicts

If `repo_pull` shows conflicts:

```bash
# Extract updates
tar -xzf updates.tar.gz

# Git will show conflicts
git status

# Resolve manually
vim src/conflicted_file.rs

# Complete merge
git add .
git commit -m "Merge remote changes"
```

### 5. Creating Pull Requests

After pushing a feature branch, you might use GitHub MCP server just for PR creation:

```text
# Use git-proxy-mcp for the code work
repo_clone → work → repo_push to feature/my-feature

# Use GitHub MCP for PR creation (it's good at this)
create_pull_request(base: "main", head: "feature/my-feature", ...)
```

Best of both worlds!
