#!/usr/bin/env bash
# Rebuilds the test fixture repository from scratch.
#
# This script nukes the remote test-dummy repo contents and recreates
# a deterministic structure for integration tests. Run before the
# integration test script to ensure a clean, known state.
#
# Requires:
#   - TEST_REPO_URL env var
#   - git credentials configured (PAT with write access)
#
# Creates:
#   - 2 commits on main
#   - 2 tags (v0.1.0, v0.2.0)
#   - 5 files: README.md, src/main.rs, src/lib.rs, docs/DESIGN.md, docs/pixel.png
#   - Exports V1_SHA and V2_SHA to /tmp/mcp-test/fixture-shas.env

set -euo pipefail

if [[ -z "${TEST_REPO_URL:-}" ]]; then
    echo "FATAL: TEST_REPO_URL not set"
    exit 1
fi

echo "Rebuilding test fixture: $TEST_REPO_URL"

WORK_DIR="/tmp/mcp-test/fixture"
rm -rf "$WORK_DIR"
mkdir -p "$WORK_DIR"
cd "$WORK_DIR"

# Clone existing repo (or init fresh if empty)
if git clone "$TEST_REPO_URL" repo 2>/dev/null; then
    cd repo

    # Delete all remote tags
    for tag in $(git tag -l); do
        git push origin --delete "$tag" 2>/dev/null || true
    done

    # Force-push an orphan branch to nuke all history
    git checkout --orphan fresh
    git rm -rf . 2>/dev/null || true
    git clean -fdx
else
    # Repo is empty — init fresh
    mkdir repo && cd repo
    git init
    git remote add origin "$TEST_REPO_URL"
    git checkout -b fresh
fi

# --- Commit 1: initial structure ---

cat > README.md << 'EOF'
# git-proxy-mcp-test-dummy

Test fixture repository for git-proxy-mcp integration tests.

**Do not modify manually.** This repo is rebuilt by CI on every integration test run.
EOF

mkdir -p src docs

cat > src/main.rs << 'EOF'
fn main() {
    println!("Hello from test fixture!");
}
EOF

cat > src/lib.rs << 'EOF'
/// A simple test function.
pub fn add(a: i32, b: i32) -> i32 {
    a + b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add() {
        assert_eq!(add(2, 3), 5);
    }
}
EOF

cat > docs/DESIGN.md << 'EOF'
# Design

This file exists to test multi-directory repository cloning.
EOF

# Small 1x1 red PNG for binary file testing
printf '\x89PNG\r\n\x1a\n' > docs/pixel.png
printf '\x00\x00\x00\rIHDR\x00\x00\x00\x01\x00\x00\x00\x01' >> docs/pixel.png
printf '\x08\x02\x00\x00\x00\x90wS\xde' >> docs/pixel.png
printf '\x00\x00\x00\x0cIDATx\x9cc\xf8\x0f\x00' >> docs/pixel.png
printf '\x00\x01\x01\x00\x05\x18\xd8N' >> docs/pixel.png
printf '\x00\x00\x00\x00IEND\xaeB\x60\x82' >> docs/pixel.png

git add -A
git commit -m "Initial commit: test fixture with src, docs, and binary file"

V1_SHA=$(git rev-parse HEAD)
echo "Commit 1 (v0.1.0): $V1_SHA"

# --- Commit 2: add subtract function ---

cat >> src/lib.rs << 'EOF'

/// Subtract two numbers.
pub fn subtract(a: i32, b: i32) -> i32 {
    a - b
}
EOF

echo "Second commit for diff testing." >> docs/DESIGN.md

git add -A
git commit -m "Add subtract function and update docs"

V2_SHA=$(git rev-parse HEAD)
echo "Commit 2 (v0.2.0): $V2_SHA"

# --- Tags ---

git tag v0.1.0 "$V1_SHA"
git tag v0.2.0 "$V2_SHA"

# --- Push (force to overwrite any existing content) ---

# Rename branch to main and force-push
git branch -M main
git push --force origin main
git push --force --tags origin

echo ""
echo "Fixture rebuilt successfully:"
echo "  v0.1.0 = $V1_SHA"
echo "  v0.2.0 = $V2_SHA"

# Export SHAs for the test script
mkdir -p /tmp/mcp-test
cat > /tmp/mcp-test/fixture-shas.env << ENVEOF
V1_SHA=$V1_SHA
V2_SHA=$V2_SHA
ENVEOF
