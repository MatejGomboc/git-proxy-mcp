#!/usr/bin/env python3
"""Rebuild the test fixture repository from scratch.

Nukes the remote test-dummy repo contents and recreates a deterministic
structure for integration tests. Run before the integration test script
to ensure a clean, known state.

Requires:
    - TEST_REPO_URL environment variable
    - git credentials configured (PAT with write access)

Creates:
    - 2 commits on main
    - 2 tags (v0.1.0, v0.2.0)
    - 5 files: README.md, src/main.rs, src/lib.rs, docs/DESIGN.md, docs/pixel.png
    - Exports V1_SHA and V2_SHA to /tmp/mcp-test/fixture-shas.env
"""

import os
import shutil
import subprocess
import sys
import tempfile

REPO_URL = os.environ.get("TEST_REPO_URL", "")

# File contents for the fixture repository.
README_CONTENT = """\
# git-proxy-mcp-test-dummy

Test fixture repository for git-proxy-mcp integration tests.

**Do not modify manually.** This repo is rebuilt by CI on every integration test run.
"""

MAIN_RS_CONTENT = """\
fn main() {
    println!("Hello from test fixture!");
}
"""

LIB_RS_INITIAL = """\
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
"""

LIB_RS_ADDITION = """
/// Subtract two numbers.
pub fn subtract(a: i32, b: i32) -> i32 {
    a - b
}
"""

DESIGN_MD_CONTENT = """\
# Design

This file exists to test multi-directory repository cloning.
"""

# Minimal valid 1x1 red PNG (67 bytes).
PIXEL_PNG = (
    b"\x89PNG\r\n\x1a\n"
    b"\x00\x00\x00\rIHDR\x00\x00\x00\x01\x00\x00\x00\x01"
    b"\x08\x02\x00\x00\x00\x90wS\xde"
    b"\x00\x00\x00\x0cIDATx\x9cc\xf8\x0f\x00"
    b"\x00\x01\x01\x00\x05\x18\xd8N"
    b"\x00\x00\x00\x00IEND\xaeB`\x82"
)


def run(args, cwd=None, check=True):
    """Run a subprocess and return its output."""
    result = subprocess.run(
        args,
        cwd=cwd,
        capture_output=True,
        text=True,
        check=check,
    )
    return result.stdout.strip()


def get_sha(cwd):
    """Get the current HEAD commit SHA."""
    return run(["git", "rev-parse", "HEAD"], cwd=cwd)


def write_file(path, content, binary=False):
    """Write content to a file, creating parent directories."""
    os.makedirs(os.path.dirname(path), exist_ok=True)
    mode = "wb" if binary else "w"
    newline = None if binary else "\n"
    with open(path, mode, newline=newline) as f:
        f.write(content)


def main():
    if not REPO_URL:
        print("FATAL: TEST_REPO_URL not set")
        sys.exit(1)

    print(f"Rebuilding test fixture: {REPO_URL}")

    work_dir = os.path.join(tempfile.gettempdir(), "mcp-test", "fixture")
    shutil.rmtree(work_dir, ignore_errors=True)
    os.makedirs(work_dir)

    repo_dir = os.path.join(work_dir, "repo")

    # Clone existing repo or init fresh.
    clone_result = subprocess.run(
        ["git", "clone", REPO_URL, "repo"],
        cwd=work_dir,
        capture_output=True,
        text=True,
    )

    if clone_result.returncode == 0:
        # Delete all tags (remote and local).
        tags = run(["git", "tag", "-l"], cwd=repo_dir)
        for tag in tags.splitlines():
            tag = tag.strip()
            if tag:
                run(["git", "push", "origin", "--delete", tag], cwd=repo_dir, check=False)
                run(["git", "tag", "-d", tag], cwd=repo_dir, check=False)

        # Create orphan branch to nuke all history.
        run(["git", "checkout", "--orphan", "fresh"], cwd=repo_dir)
        run(["git", "rm", "-rf", "."], cwd=repo_dir, check=False)
    else:
        # Repo is empty — init fresh.
        os.makedirs(repo_dir)
        run(["git", "init"], cwd=repo_dir)
        run(["git", "remote", "add", "origin", REPO_URL], cwd=repo_dir)
        run(["git", "checkout", "-b", "fresh"], cwd=repo_dir)

    # --- Commit 1: initial structure ---
    write_file(os.path.join(repo_dir, "README.md"), README_CONTENT)
    write_file(os.path.join(repo_dir, "src", "main.rs"), MAIN_RS_CONTENT)
    write_file(os.path.join(repo_dir, "src", "lib.rs"), LIB_RS_INITIAL)
    write_file(os.path.join(repo_dir, "docs", "DESIGN.md"), DESIGN_MD_CONTENT)
    write_file(os.path.join(repo_dir, "docs", "pixel.png"), PIXEL_PNG, binary=True)

    run(["git", "add", "-A"], cwd=repo_dir)
    run(
        ["git", "commit", "-m", "Initial commit: test fixture with src, docs, and binary file"],
        cwd=repo_dir,
    )

    v1_sha = get_sha(repo_dir)
    print(f"Commit 1 (v0.1.0): {v1_sha}")

    # --- Commit 2: add subtract function ---
    lib_path = os.path.join(repo_dir, "src", "lib.rs")
    with open(lib_path, "a", newline="\n") as f:
        f.write(LIB_RS_ADDITION)

    design_path = os.path.join(repo_dir, "docs", "DESIGN.md")
    with open(design_path, "a", newline="\n") as f:
        f.write("Second commit for diff testing.\n")

    run(["git", "add", "-A"], cwd=repo_dir)
    run(["git", "commit", "-m", "Add subtract function and update docs"], cwd=repo_dir)

    v2_sha = get_sha(repo_dir)
    print(f"Commit 2 (v0.2.0): {v2_sha}")

    # --- Tags ---
    run(["git", "tag", "v0.1.0", v1_sha], cwd=repo_dir)
    run(["git", "tag", "v0.2.0", v2_sha], cwd=repo_dir)

    # --- Push (force to overwrite any existing content) ---
    run(["git", "branch", "-M", "main"], cwd=repo_dir)
    run(["git", "push", "--force", "origin", "main"], cwd=repo_dir)
    run(["git", "push", "--force", "--tags", "origin"], cwd=repo_dir)

    print()
    print("Fixture rebuilt successfully:")
    print(f"  v0.1.0 = {v1_sha}")
    print(f"  v0.2.0 = {v2_sha}")

    # Export SHAs for the test script.
    env_dir = os.path.join(tempfile.gettempdir(), "mcp-test")
    os.makedirs(env_dir, exist_ok=True)
    env_path = os.path.join(env_dir, "fixture-shas.env")
    with open(env_path, "w", newline="\n") as f:
        f.write(f"V1_SHA={v1_sha}\n")
        f.write(f"V2_SHA={v2_sha}\n")


if __name__ == "__main__":
    main()
