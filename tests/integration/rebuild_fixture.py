#!/usr/bin/env python3
"""Rebuild the test fixture repository from scratch.

Nukes the remote test-dummy repo contents and recreates a deterministic
structure for integration tests. Run before the integration test script
to ensure a clean, known state.

Requires:
    - TEST_REPO_URL environment variable
    - git credentials configured (PAT with write access)

Creates:
    - 3 commits on main
    - 2 tags (v0.1.0, v0.2.0)
    - 5 source files + 20 generated data files + 1 submodule
    - Exports V1_SHA, V2_SHA, V3_SHA to /tmp/mcp-test/fixture-shas.env
"""

import os
import re
import shutil
import subprocess
import sys
import tempfile
from urllib.parse import urlsplit, urlunsplit

_HOST_RE = re.compile(r"\A[A-Za-z0-9.\-]+\Z")
_PATH_RE = re.compile(r"\A/[A-Za-z0-9._\-/]+\.git\Z")


def _sanitise_repo_url(raw: str) -> str:
    """Validate and reconstruct the repo URL from its parts.

    Parsing with `urlsplit` and rebuilding via `urlunsplit` produces a
    new string that CodeQL recognises as sanitised, closing the
    `py/command-line-injection` alert on the `subprocess.run` calls
    that use the result.

    Only plain `https://host/path.git` URLs are accepted; anything
    else (including SSH URLs, query strings, userinfo, or unusual
    characters) is rejected.
    """
    parts = urlsplit(raw)
    if parts.scheme != "https":
        raise ValueError(f"TEST_REPO_URL must use https (got {parts.scheme!r})")
    if parts.username or parts.password or parts.port:
        raise ValueError("TEST_REPO_URL must not contain userinfo or port")
    if parts.query or parts.fragment:
        raise ValueError("TEST_REPO_URL must not contain query or fragment")
    if not _HOST_RE.fullmatch(parts.hostname or ""):
        raise ValueError(f"TEST_REPO_URL has invalid host: {parts.hostname!r}")
    if not _PATH_RE.fullmatch(parts.path):
        raise ValueError(f"TEST_REPO_URL has invalid path: {parts.path!r}")
    return urlunsplit(("https", parts.hostname, parts.path, "", ""))


REPO_URL = _sanitise_repo_url(os.environ["TEST_REPO_URL"]) if os.environ.get("TEST_REPO_URL") else ""

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
    encoding = None if binary else "utf-8"
    with open(path, mode, newline=newline, encoding=encoding) as f:
        f.write(content)


def main():
    """Rebuild the test fixture repository from scratch."""
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

    # --- Commit 2: add subtract function + generated data files ---
    lib_path = os.path.join(repo_dir, "src", "lib.rs")
    with open(lib_path, "a", newline="\n", encoding="utf-8") as f:
        f.write(LIB_RS_ADDITION)

    design_path = os.path.join(repo_dir, "docs", "DESIGN.md")
    with open(design_path, "a", newline="\n", encoding="utf-8") as f:
        f.write("Second commit for diff testing.\n")

    # Generate data files to make the archive large enough for
    # multi-chunk streaming tests. Minimum chunk_size is 1024, so we
    # need the compressed archive to exceed 2048 bytes for 3+ chunks.
    # Each file is ~200 bytes, 40 files = ~8KB uncompressed.
    data_dir = os.path.join(repo_dir, "data")
    os.makedirs(data_dir, exist_ok=True)
    for i in range(40):
        content = f"# Data file {i:03d}\n" + f"Line {i} generated content.\n" * 6
        write_file(os.path.join(data_dir, f"file_{i:03d}.txt"), content)

    run(["git", "add", "-A"], cwd=repo_dir)
    run(
        ["git", "commit", "-m", "Add subtract function, docs, and data files"],
        cwd=repo_dir,
    )

    v2_sha = get_sha(repo_dir)
    print(f"Commit 2 (v0.2.0): {v2_sha}")

    # --- Commit 3: add a submodule ---
    # Use a small, stable public repo as a submodule.
    submodule_url = "https://github.com/nickel-org/rust-mustache.git"
    run(
        ["git", "submodule", "add", submodule_url, "vendor/mustache"],
        cwd=repo_dir,
    )
    run(["git", "commit", "-m", "Add submodule for integration testing"], cwd=repo_dir)

    v3_sha = get_sha(repo_dir)
    print(f"Commit 3 (HEAD): {v3_sha}")

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
    print(f"  HEAD   = {v3_sha}")

    # Export SHAs for the test script.
    env_dir = os.path.join(tempfile.gettempdir(), "mcp-test")
    os.makedirs(env_dir, exist_ok=True)
    env_path = os.path.join(env_dir, "fixture-shas.env")
    with open(env_path, "w", newline="\n", encoding="utf-8") as f:
        f.write(f"V1_SHA={v1_sha}\n")
        f.write(f"V2_SHA={v2_sha}\n")
        f.write(f"V3_SHA={v3_sha}\n")


if __name__ == "__main__":
    main()
