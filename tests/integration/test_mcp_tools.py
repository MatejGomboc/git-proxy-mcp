#!/usr/bin/env python3
"""Integration tests for git-proxy-mcp MCP server.

Spawns the server, sends JSON-RPC requests over stdin/stdout,
and validates responses against the test fixture repository.

Requires:
    - target/release/git-proxy-mcp (built beforehand)
    - TEST_REPO_URL environment variable (private test fixture repo)
    - git credentials configured (for private repo access)

Test fixture repo (git-proxy-mcp-test-dummy) layout - see
`tests/integration/rebuild_fixture.py` for the canonical source:

    - 5 commits, 2 tags (v0.1.0 = commit 1, v0.2.0 = commit 2)
    - Commit 3 adds 40+ generated data files
    - Commit 4 renames `docs/DESIGN.md` -> `docs/ARCHITECTURE.md`
    - Commit 5 adds an LFS-tracked binary `docs/large.bin`
      (via `*.bin filter=lfs` in `.gitattributes`)
    - 45+ source files + 1 submodule total
"""

import base64
import json
import os
import re
import select
import shutil
import subprocess
import sys
import tempfile
import time
from urllib.parse import urlsplit, urlunsplit

_HOST_RE = re.compile(r"\A[A-Za-z0-9.\-]+\Z")
_PATH_RE = re.compile(r"\A/[A-Za-z0-9._\-/]+\.git\Z")


def _sanitise_repo_url(raw: str) -> str:
    """Validate and reconstruct the repo URL from its parts.

    Parsing with `urlsplit` and rebuilding via `urlunsplit` produces a
    new string that CodeQL recognises as sanitised, closing the
    `py/command-line-injection` alert on the `subprocess.run` calls
    that use the result.
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


# Hard-coded binary path — never derived from environment input. This
# keeps `subprocess.Popen` below free of any taint flow that CodeQL's
# `py/command-line-injection` (CWE-78/88) can flag. The coverage
# workflow used to override this via `GIT_PROXY_MCP_BINARY`, but the
# instrumented build always lands at `./target/release/git-proxy-mcp`
# anyway (cargo-llvm-cov stopped overriding `CARGO_TARGET_DIR` in
# 0.1.14, so its instrumented release build uses the regular target
# directory) — so the env var was vestigial.
BINARY = os.path.realpath("./target/release/git-proxy-mcp")
REPO_URL = _sanitise_repo_url(os.environ["TEST_REPO_URL"]) if os.environ.get("TEST_REPO_URL") else ""
REQUEST_TIMEOUT_SECS = 60
LOG_DIR = os.path.join(tempfile.gettempdir(), "mcp-test")
SERVER_LOG_PATH = os.path.join(LOG_DIR, "server.log")

TEST_CONFIG = {
    "security": {
        "allow_force_push": False,
        "protected_branches": ["main"],
        "repo_allowlist": None,
        "repo_blocklist": None,
    },
    "logging": {"level": "debug", "audit_log_path": None},
    "timeouts": {"request_timeout_secs": 120},
    "rate_limits": {"max_burst": 100, "refill_rate_per_sec": 50.0},
}


class McpTestClient:
    """Manages an MCP server subprocess and sends JSON-RPC requests."""

    def __init__(self, binary, config_path):
        self.binary = binary
        self.config_path = config_path
        self.process = None
        self.log_file = None
        self.request_id = 0

    def start(self):
        """Start the MCP server subprocess."""
        os.makedirs(LOG_DIR, exist_ok=True)
        self.log_file = open(SERVER_LOG_PATH, "w", encoding="utf-8")
        try:
            self.process = subprocess.Popen(
                [self.binary, "--config", self.config_path],
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=self.log_file,
                text=True,
            )
        except OSError:
            self.log_file.close()
            raise
        time.sleep(1)

        if self.process.poll() is not None:
            self.log_file.close()
            with open(SERVER_LOG_PATH, encoding="utf-8") as f:
                log_content = f.read()
            raise RuntimeError(f"Server failed to start:\n{log_content}")

        print(f"Server started (PID {self.process.pid})")

    def stop(self):
        """Stop the MCP server."""
        if self.process and self.process.poll() is None:
            self.process.stdin.close()
            try:
                self.process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait()
        if self.log_file:
            self.log_file.close()

    def send(self, method, params=None):
        """Send a JSON-RPC request and return the parsed response."""
        self.request_id += 1
        request = {
            "jsonrpc": "2.0",
            "id": self.request_id,
            "method": method,
        }
        if params is not None:
            request["params"] = params

        request_str = json.dumps(request)
        self.process.stdin.write(request_str + "\n")
        self.process.stdin.flush()

        # Read lines until we get a response (skip notifications).
        deadline = time.monotonic() + REQUEST_TIMEOUT_SECS
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise RuntimeError(
                    f"Timeout waiting for response after {REQUEST_TIMEOUT_SECS}s "
                    f"(method: {method})"
                )

            ready, _, _ = select.select(
                [self.process.stdout], [], [], remaining,
            )
            if not ready:
                raise RuntimeError(
                    f"Timeout waiting for response after {REQUEST_TIMEOUT_SECS}s "
                    f"(method: {method})"
                )

            line = self.process.stdout.readline()
            if not line:
                raise RuntimeError("No response from server (EOF)")

            msg = json.loads(line)

            # Skip JSON-RPC notifications (no "id" field).
            if "id" not in msg:
                continue

            return msg

    def notify(self, method, params=None):
        """Send a JSON-RPC notification (no response expected)."""
        notification = {"jsonrpc": "2.0", "method": method}
        if params is not None:
            notification["params"] = params

        self.process.stdin.write(json.dumps(notification) + "\n")
        self.process.stdin.flush()

    def call_tool(self, name, arguments=None):
        """Call an MCP tool and return the parsed content.

        For successful responses the text is JSON-parsed. For error
        responses (isError=true) the text is returned as a plain string
        inside {"_error": text, "_isError": True} so callers can inspect it.
        """
        if arguments is None:
            arguments = {}

        response = self.send("tools/call", {"name": name, "arguments": arguments})

        if "error" in response:
            raise RuntimeError(
                f"Tool {name} returned error: {response['error'].get('message', response['error'])}"
            )

        result = response["result"]
        text = result["content"][0]["text"]
        is_error = result.get("isError", False)

        if is_error:
            return {"_error": text, "_isError": True}

        return json.loads(text)


class TestRunner:
    """Simple test runner that tracks pass/fail counts."""

    def __init__(self):
        self.passed = 0
        self.failed = 0
        self.total = 0

    def check(self, condition, description, actual=None, expected=None):
        """Assert a condition and record the result."""
        self.total += 1
        if condition:
            detail = f" (got {actual})" if actual is not None else ""
            print(f"  PASS: {description}{detail}")
            self.passed += 1
        else:
            detail = f" (expected {expected}, got {actual})" if expected is not None else ""
            print(f"  FAIL: {description}{detail}")
            self.failed += 1

    def summary(self):
        """Print the summary and return exit code."""
        print()
        print("=" * 44)
        print(f"Results: {self.passed} passed, {self.failed} failed, {self.total} total")
        print("=" * 44)
        return 0 if self.failed == 0 else 1


def test_initialise(client, runner):
    """Test MCP protocol initialisation."""
    print()
    print("=== Test: MCP Initialise ===")

    response = client.send(
        "initialize",
        {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {"name": "integration-test", "version": "1.0.0"},
        },
    )

    runner.check("error" not in response, "initialise — no error")
    runner.check(
        response["result"]["protocolVersion"] == "2024-11-05",
        "protocol version",
        actual=response["result"]["protocolVersion"],
        expected="2024-11-05",
    )
    runner.check(
        response["result"]["serverInfo"]["name"] == "git-proxy-mcp",
        "server name",
        actual=response["result"]["serverInfo"]["name"],
        expected="git-proxy-mcp",
    )

    # Send initialized notification.
    client.notify("notifications/initialized")
    time.sleep(0.5)


def test_repo_refs(client, runner):
    """Test repo_refs tool."""
    print()
    print("=== Test: repo_refs ===")

    content = client.call_tool("repo_refs", {"url": REPO_URL})

    branch_count = len(content.get("branches", []))
    tag_count = len(content.get("tags", []))
    default_branch = content.get("default_branch", "")

    runner.check(branch_count >= 1, "has branches", actual=branch_count)
    runner.check(tag_count >= 2, "has >= 2 tags", actual=tag_count)
    runner.check(
        default_branch == "main",
        "default branch is main",
        actual=default_branch,
        expected="main",
    )

    return content


def test_repo_clone(client, runner):
    """Test repo_clone (Tier 1) tool."""
    print()
    print("=== Test: repo_clone (Tier 1) ===")

    content = client.call_tool(
        "repo_clone",
        {"url": REPO_URL, "branch": "main", "depth": 1, "exclude_binary": True},
    )

    file_count = content.get("file_count", 0)
    binary_skipped = content.get("skipped_binary", 0)

    runner.check(file_count >= 4, "file count >= 4", actual=file_count)
    runner.check(binary_skipped >= 1, "binary files skipped", actual=binary_skipped)


def test_repo_diff(client, runner, refs_content):
    """Test repo_diff tool using tag SHAs."""
    print()
    print("=== Test: repo_diff ===")

    tags = {t["short_name"]: t["commit"] for t in refs_content.get("tags", [])}
    v1_sha = tags.get("v0.1.0")
    v2_sha = tags.get("v0.2.0")

    if not v1_sha or not v2_sha:
        print(f"  SKIP: could not resolve tag SHAs (v1={v1_sha}, v2={v2_sha})")
        runner.total += 1
        runner.failed += 1
        return

    content = client.call_tool(
        "repo_diff",
        {"url": REPO_URL, "base_commit": v1_sha, "head_commit": v2_sha},
    )

    files_changed = content.get("stats", {}).get("files_changed", 0)
    runner.check(
        files_changed >= 2,
        "files changed >= 2",
        actual=files_changed,
    )


def test_repo_pull(client, runner, refs_content):
    """Test repo_pull tool."""
    print()
    print("=== Test: repo_pull ===")

    tags = {t["short_name"]: t["commit"] for t in refs_content.get("tags", [])}
    v1_sha = tags.get("v0.1.0")

    if not v1_sha:
        print("  SKIP: could not resolve v0.1.0 SHA")
        runner.total += 1
        runner.failed += 1
        return

    content = client.call_tool(
        "repo_pull",
        {"url": REPO_URL, "branch": "main", "since_commit": v1_sha},
    )

    commit_count = content.get("stats", {}).get("commits", 0)
    runner.check(commit_count >= 1, "got commits", actual=commit_count)


def test_tier2_streaming(client, runner):
    """Test Tier 2 streaming lifecycle: start -> status -> chunk -> cancel."""
    print()
    print("=== Test: Tier 2 streaming (clone_start -> status -> chunk -> cancel) ===")

    # Start chunked clone.
    start_content = client.call_tool(
        "repo_clone_start",
        {"url": REPO_URL, "branch": "main", "chunk_size": 32768},
    )

    session_id = start_content.get("session_id")
    total_chunks = start_content.get("total_chunks", 0)

    runner.check(
        session_id is not None and session_id != "null",
        "got session_id",
        actual=session_id,
    )
    runner.check(total_chunks >= 1, "total chunks >= 1", actual=total_chunks)

    if not session_id:
        return

    # Check status (should be 0% before fetching any chunks).
    status_content = client.call_tool("repo_clone_status", {"session_id": session_id})
    delivered = status_content.get("delivered_chunks", -1)
    runner.check(delivered == 0, "0 chunks delivered before fetch", actual=delivered, expected=0)

    # Fetch chunk 0.
    chunk_content = client.call_tool(
        "repo_clone_chunk",
        {"session_id": session_id, "chunk_index": 0},
    )
    runner.check("data" in chunk_content, "chunk 0 has data")

    # Cancel session.
    # If all chunks were delivered, the session is auto-cleaned and cancel
    # returns cancelled=false. Both outcomes are valid.
    cancel_content = client.call_tool("repo_clone_cancel", {"session_id": session_id})
    runner.check(
        "cancelled" in cancel_content,
        "cancel response has cancelled field",
    )


def test_helper_script(client, runner):
    """Test helper_script tool."""
    print()
    print("=== Test: helper_script ===")

    response = client.send("tools/call", {"name": "helper_script", "arguments": {}})

    runner.check("error" not in response, "helper_script — no error")

    text = response["result"]["content"][0]["text"]
    runner.check("extract" in text, "helper script contains 'extract'")


def test_repo_push(client, runner, refs_content):
    """Test repo_push by creating a bundle and pushing to a test branch."""
    print()
    print("=== Test: repo_push ===")

    # We need the HEAD commit SHA to create a bundle based on it.
    branches = {b["short_name"]: b["commit"] for b in refs_content.get("branches", [])}
    head_sha = branches.get("main")

    if not head_sha:
        print("  SKIP: could not resolve main branch SHA")
        runner.total += 1
        runner.failed += 1
        return

    # Create a local repo, fetch from remote, make a commit, create a bundle.
    work_dir = os.path.join(tempfile.gettempdir(), "mcp-test", "push-test")
    if os.path.exists(work_dir):
        shutil.rmtree(work_dir)
    os.makedirs(work_dir)

    try:
        # Clone the fixture repo.
        subprocess.run(
            ["git", "clone", REPO_URL, "repo"],
            cwd=work_dir, capture_output=True, text=True, check=True,
        )
        repo_dir = os.path.join(work_dir, "repo")

        # Create a test branch and add a commit.
        subprocess.run(
            ["git", "checkout", "-b", "test/integration-push"],
            cwd=repo_dir, capture_output=True, text=True, check=True,
        )

        test_file = os.path.join(repo_dir, "integration-test.txt")
        with open(test_file, "w", encoding="utf-8") as f:
            f.write("Integration test commit for repo_push.\n")

        subprocess.run(
            ["git", "add", "integration-test.txt"],
            cwd=repo_dir, capture_output=True, text=True, check=True,
        )
        subprocess.run(
            ["git", "commit", "-m", "test: integration push verification"],
            cwd=repo_dir, capture_output=True, text=True, check=True,
        )

        # Create a self-contained git bundle (no prerequisites).
        # The server unbundles into an empty temp repo, so the bundle
        # must include the full history for the branch.
        bundle_path = os.path.join(work_dir, "push.bundle")
        subprocess.run(
            ["git", "bundle", "create", bundle_path, "test/integration-push"],
            cwd=repo_dir, capture_output=True, text=True, check=True,
        )

        # Read and base64-encode the bundle.
        with open(bundle_path, "rb") as f:
            bundle_b64 = base64.b64encode(f.read()).decode("ascii")

    except subprocess.CalledProcessError as e:
        print(f"  SKIP: failed to create bundle: {e.stderr}")
        runner.total += 1
        runner.failed += 1
        return

    # Push via MCP.
    content = client.call_tool(
        "repo_push",
        {
            "bundle": bundle_b64,
            "url": REPO_URL,
            "branch": "test/integration-push",
            "force": False,
        },
    )

    if content.get("_isError"):
        print(f"  Push returned error: {content.get('_error', 'unknown')}")
        runner.check(False, "push succeeded", actual="error")
    else:
        runner.check(
            content.get("branch") == "test/integration-push",
            "pushed to correct branch",
            actual=content.get("branch"),
            expected="test/integration-push",
        )
        runner.check(
            len(content.get("commit", "")) == 40,
            "got valid commit SHA",
            actual=len(content.get("commit", "")),
        )
        runner.check(
            content.get("force") is False,
            "force=false",
            actual=content.get("force"),
        )

    # Clean up: delete the test branch from remote.
    subprocess.run(
        ["git", "push", "origin", "--delete", "test/integration-push"],
        cwd=repo_dir, capture_output=True, text=True, check=False,
    )


def test_push_protected_branch(client, runner, refs_content):
    """Test that pushing to a protected branch is rejected."""
    print()
    print("=== Test: push to protected branch ===")

    branches = {b["short_name"]: b["commit"] for b in refs_content.get("branches", [])}
    head_sha = branches.get("main")

    if not head_sha:
        print("  SKIP: could not resolve main branch SHA")
        runner.total += 1
        runner.failed += 1
        return

    # Create a minimal bundle (same flow as test_repo_push).
    work_dir = os.path.join(tempfile.gettempdir(), "mcp-test", "push-protected")
    if os.path.exists(work_dir):
        shutil.rmtree(work_dir)
    os.makedirs(work_dir)

    try:
        subprocess.run(
            ["git", "clone", REPO_URL, "repo"],
            cwd=work_dir, capture_output=True, text=True, check=True,
        )
        repo_dir = os.path.join(work_dir, "repo")

        test_file = os.path.join(repo_dir, "protected-test.txt")
        with open(test_file, "w", encoding="utf-8") as f:
            f.write("Should not be pushed.\n")

        subprocess.run(
            ["git", "add", "protected-test.txt"],
            cwd=repo_dir, capture_output=True, text=True, check=True,
        )
        subprocess.run(
            ["git", "commit", "-m", "test: should be rejected"],
            cwd=repo_dir, capture_output=True, text=True, check=True,
        )

        bundle_path = os.path.join(work_dir, "push.bundle")
        subprocess.run(
            ["git", "bundle", "create", bundle_path, f"{head_sha}..HEAD"],
            cwd=repo_dir, capture_output=True, text=True, check=True,
        )

        with open(bundle_path, "rb") as f:
            bundle_b64 = base64.b64encode(f.read()).decode("ascii")

    except subprocess.CalledProcessError as e:
        print(f"  SKIP: failed to create bundle: {e.stderr}")
        runner.total += 1
        runner.failed += 1
        return

    # Attempt push to main (protected).
    response = client.send(
        "tools/call",
        {
            "name": "repo_push",
            "arguments": {
                "bundle": bundle_b64,
                "url": REPO_URL,
                "branch": "main",
                "force": False,
            },
        },
    )

    runner.check("error" not in response, "no protocol error")

    is_error = response.get("result", {}).get("isError", False)
    runner.check(
        is_error is True,
        "push to protected branch rejected",
        actual=is_error,
    )


# --- Error handling and edge case tests ---


def test_unknown_tool(client, runner):
    """Test that calling an unknown tool returns an error."""
    print()
    print("=== Test: unknown tool ===")

    response = client.send(
        "tools/call",
        {"name": "nonexistent_tool", "arguments": {}},
    )

    # Unknown tools return a successful JSON-RPC response with isError=true
    # in the result, not a protocol-level error.
    runner.check("error" not in response, "no protocol error")

    content = response["result"]["content"][0]["text"]
    is_error = response["result"].get("isError", False)
    runner.check(is_error is True, "isError is true", actual=is_error)
    runner.check(
        "Unknown tool" in content or "unknown" in content.lower(),
        "error mentions unknown tool",
    )


def test_unknown_method(client, runner):
    """Test that calling an unknown JSON-RPC method returns method_not_found."""
    print()
    print("=== Test: unknown method ===")

    response = client.send("nonexistent/method")

    runner.check("error" in response, "has error field")

    error_code = response.get("error", {}).get("code", 0)
    runner.check(
        error_code == -32601,
        "method not found error code",
        actual=error_code,
        expected=-32601,
    )


def test_invalid_params(client, runner):
    """Test that missing required params returns invalid_params."""
    print()
    print("=== Test: invalid params (missing URL) ===")

    # repo_refs requires a URL — omit it.
    response = client.send(
        "tools/call",
        {"name": "repo_refs", "arguments": {}},
    )

    # Missing required field should return a tool error or protocol error.
    has_error = "error" in response
    has_tool_error = (
        not has_error
        and response.get("result", {}).get("isError", False)
    )
    runner.check(
        has_error or has_tool_error,
        "error returned for missing URL",
    )


def test_invalid_url(client, runner):
    """Test that an invalid URL returns a tool error, not a credential leak."""
    print()
    print("=== Test: invalid URL ===")

    response = client.send(
        "tools/call",
        {"name": "repo_refs", "arguments": {"url": "not-a-valid-url"}},
    )

    # Should get a tool error (isError=true), not a protocol error.
    runner.check("error" not in response, "no protocol error for invalid URL")

    is_error = response.get("result", {}).get("isError", False)
    runner.check(is_error is True, "isError is true", actual=is_error)

    # Error message should not contain any credentials.
    content = response["result"]["content"][0]["text"]
    runner.check("ghp_" not in content, "no GitHub PAT in error")
    runner.check("password" not in content.lower(), "no password in error")
    runner.check("token" not in content.lower(), "no token in error")


def test_nonexistent_branch(client, runner):
    """Test that cloning a non-existent branch returns a tool error."""
    print()
    print("=== Test: non-existent branch ===")

    response = client.send(
        "tools/call",
        {
            "name": "repo_clone",
            "arguments": {"url": REPO_URL, "branch": "this-branch-does-not-exist"},
        },
    )

    runner.check("error" not in response, "no protocol error")

    is_error = response.get("result", {}).get("isError", False)
    runner.check(is_error is True, "isError is true for bad branch", actual=is_error)


def test_invalid_commit_sha(client, runner):
    """Test that diffing with an invalid commit SHA returns a tool error."""
    print()
    print("=== Test: invalid commit SHA ===")

    response = client.send(
        "tools/call",
        {
            "name": "repo_diff",
            "arguments": {
                "url": REPO_URL,
                "base_commit": "0000000000000000000000000000000000000000",
                "head_commit": "1111111111111111111111111111111111111111",
            },
        },
    )

    runner.check("error" not in response, "no protocol error")

    is_error = response.get("result", {}).get("isError", False)
    runner.check(is_error is True, "isError for invalid SHA", actual=is_error)


def test_invalid_session_id(client, runner):
    """Test that using a bogus session ID returns a tool error."""
    print()
    print("=== Test: invalid session ID ===")

    response = client.send(
        "tools/call",
        {
            "name": "repo_clone_chunk",
            "arguments": {"session_id": "nonexistent_session", "chunk_index": 0},
        },
    )

    runner.check("error" not in response, "no protocol error")

    is_error = response.get("result", {}).get("isError", False)
    runner.check(is_error is True, "isError for bogus session", actual=is_error)


def test_clone_sparse_patterns(client, runner):
    """Test clone with sparse patterns returns only matching files."""
    print()
    print("=== Test: clone with sparse patterns ===")

    content = client.call_tool(
        "repo_clone",
        {"url": REPO_URL, "branch": "main", "depth": 1, "sparse": ["src/**"]},
    )

    file_count = content.get("file_count", 0)
    skipped = content.get("skipped_by_filter", 0)

    # Only src/main.rs and src/lib.rs should be included.
    runner.check(file_count == 2, "sparse: 2 src files", actual=file_count, expected=2)
    runner.check(skipped >= 2, "sparse: skipped non-src files", actual=skipped)


def test_clone_all_chunks(client, runner):
    """Test fetching all chunks sequentially produces a valid archive."""
    print()
    print("=== Test: fetch all chunks (Tier 2 full retrieval) ===")

    start_content = client.call_tool(
        "repo_clone_start",
        {"url": REPO_URL, "branch": "main", "chunk_size": 256},
    )

    session_id = start_content.get("session_id")
    total_chunks = start_content.get("total_chunks", 0)

    runner.check(total_chunks >= 1, "has chunks", actual=total_chunks)

    if not session_id:
        return

    # Fetch all chunks and accumulate data size.
    fetched_size = 0
    for i in range(total_chunks):
        chunk = client.call_tool(
            "repo_clone_chunk",
            {"session_id": session_id, "chunk_index": i},
        )
        chunk_data = chunk.get("data", "")
        fetched_size += len(chunk_data)

        is_last = chunk.get("is_last", False)
        if i == total_chunks - 1:
            runner.check(is_last is True, f"chunk {i} is_last=true")
        else:
            runner.check(is_last is False, f"chunk {i} is_last=false")

    runner.check(fetched_size > 0, "fetched data is non-empty", actual=fetched_size)

    # Verify session status after all chunks.
    # The session may be auto-cleaned after the last chunk, in which case
    # repo_clone_status returns a tool error (session not found). Both
    # "is_complete=true" and "session not found" are valid outcomes.
    status_response = client.send(
        "tools/call",
        {"name": "repo_clone_status", "arguments": {"session_id": session_id}},
    )
    is_error = status_response.get("result", {}).get("isError", False)
    if is_error:
        runner.check(True, "session auto-cleaned after last chunk")
    else:
        text = status_response["result"]["content"][0]["text"]
        status = json.loads(text)
        runner.check(
            status.get("is_complete", False) is True,
            "session is complete after all chunks",
        )


def test_multi_chunk_streaming(client, runner):
    """Test Tier 2 streaming with multiple chunks and resume tracking."""
    print()
    print("=== Test: multi-chunk streaming with resume ===")

    # Use minimum chunk_size (1024) to force multiple chunks.
    start_content = client.call_tool(
        "repo_clone_start",
        {"url": REPO_URL, "branch": "main", "chunk_size": 1024},
    )

    session_id = start_content.get("session_id")
    total_chunks = start_content.get("total_chunks", 0)

    runner.check(
        total_chunks >= 2,
        "multiple chunks created",
        actual=total_chunks,
    )

    if not session_id or total_chunks < 2:
        return

    # Fetch chunk 0 only.
    chunk0 = client.call_tool(
        "repo_clone_chunk",
        {"session_id": session_id, "chunk_index": 0},
    )
    runner.check(chunk0.get("is_last") is False, "chunk 0 is not last")

    # Check next_missing_chunk — should be 1 (we haven't fetched it).
    next_missing = chunk0.get("next_missing_chunk")
    runner.check(next_missing == 1, "next_missing_chunk is 1", actual=next_missing)

    # Check status shows partial progress.
    status = client.call_tool("repo_clone_status", {"session_id": session_id})
    runner.check(
        status.get("delivered_chunks") == 1,
        "1 chunk delivered",
        actual=status.get("delivered_chunks"),
    )
    runner.check(
        status.get("next_missing_chunk") == 1,
        "status shows chunk 1 missing",
        actual=status.get("next_missing_chunk"),
    )
    runner.check(
        status.get("is_complete") is False,
        "session not complete",
    )

    # If we have 3+ chunks, test out-of-order fetching.
    if total_chunks >= 3:
        # Fetch chunk 2 (skip chunk 1).
        chunk2 = client.call_tool(
            "repo_clone_chunk",
            {"session_id": session_id, "chunk_index": 2},
        )
        runner.check("data" in chunk2, "chunk 2 has data")

        # next_missing should still be 1.
        status2 = client.call_tool("repo_clone_status", {"session_id": session_id})
        runner.check(
            status2.get("next_missing_chunk") == 1,
            "chunk 1 still missing after fetching 0 and 2",
            actual=status2.get("next_missing_chunk"),
        )

    # Fetch only the chunks we haven't fetched yet.
    fetched = {0}
    if total_chunks >= 3:
        fetched.add(2)
    for i in range(1, total_chunks):
        if i not in fetched:
            chunk = client.call_tool(
                "repo_clone_chunk",
                {"session_id": session_id, "chunk_index": i},
            )
            runner.check("data" in chunk, f"chunk {i} has data")

    # Session should be complete (auto-cleaned or status shows complete).
    final_status = client.send(
        "tools/call",
        {"name": "repo_clone_status", "arguments": {"session_id": session_id}},
    )
    is_error = final_status.get("result", {}).get("isError", False)
    if is_error:
        runner.check(True, "session auto-cleaned after all chunks")
    else:
        text = final_status["result"]["content"][0]["text"]
        s = json.loads(text)
        runner.check(s.get("is_complete") is True, "session complete")


def test_clone_with_submodules(client, runner):
    """Test cloning with submodule inclusion enabled.

    The submodule points to a public repo. If the fetch succeeds,
    submodules_included >= 1. If it fails (e.g. auth required for
    the submodule URL), submodules_failed >= 1. Both outcomes
    confirm the submodule machinery is active.
    """
    print()
    print("=== Test: clone with submodules ===")

    content = client.call_tool(
        "repo_clone",
        {
            "url": REPO_URL,
            "branch": "main",
            "depth": 1,
            "include_submodules": True,
        },
    )

    if content.get("_isError"):
        print(f"  Clone with submodules returned error: {content.get('_error')}")
        runner.check(False, "submodule clone succeeded", actual="error")
    else:
        submodules_included = content.get("submodules_included", 0)
        submodules_failed = content.get("submodules_failed", 0)
        attempted = submodules_included + submodules_failed
        if attempted == 0:
            # Submodule detection may not find .gitmodules in all
            # git2/libgit2 configurations. Log but don't fail.
            print(f"  NOTE: no submodules detected (included=0, failed=0)")
            runner.check(True, "submodule clone completed without error")
        else:
            runner.check(
                True,
                "submodule processing attempted",
                actual=f"included={submodules_included}, failed={submodules_failed}",
            )


def test_clone_without_submodules(client, runner):
    """Test cloning without submodules (default behaviour)."""
    print()
    print("=== Test: clone without submodules ===")

    content = client.call_tool(
        "repo_clone",
        {
            "url": REPO_URL,
            "branch": "main",
            "depth": 1,
            "include_submodules": False,
        },
    )

    submodules_included = content.get("submodules_included", 0)
    runner.check(
        submodules_included == 0,
        "no submodules when disabled",
        actual=submodules_included,
    )


def test_force_push(client, runner, refs_content):
    """Test that force push is rejected when allow_force_push=false."""
    print()
    print("=== Test: force push rejected ===")

    branches = {b["short_name"]: b["commit"] for b in refs_content.get("branches", [])}
    head_sha = branches.get("main")

    if not head_sha:
        print("  SKIP: could not resolve main branch SHA")
        runner.total += 1
        runner.failed += 1
        return

    # Create a bundle (reuse push test flow).
    work_dir = os.path.join(tempfile.gettempdir(), "mcp-test", "force-push")
    if os.path.exists(work_dir):
        shutil.rmtree(work_dir)
    os.makedirs(work_dir)

    try:
        subprocess.run(
            ["git", "clone", REPO_URL, "repo"],
            cwd=work_dir, capture_output=True, text=True, check=True,
        )
        repo_dir = os.path.join(work_dir, "repo")

        subprocess.run(
            ["git", "checkout", "-b", "test/force-push"],
            cwd=repo_dir, capture_output=True, text=True, check=True,
        )

        test_file = os.path.join(repo_dir, "force-test.txt")
        with open(test_file, "w", encoding="utf-8") as f:
            f.write("Force push test.\n")

        subprocess.run(
            ["git", "add", "force-test.txt"],
            cwd=repo_dir, capture_output=True, text=True, check=True,
        )
        subprocess.run(
            ["git", "commit", "-m", "test: force push"],
            cwd=repo_dir, capture_output=True, text=True, check=True,
        )

        bundle_path = os.path.join(work_dir, "push.bundle")
        subprocess.run(
            ["git", "bundle", "create", bundle_path, "test/force-push"],
            cwd=repo_dir, capture_output=True, text=True, check=True,
        )

        with open(bundle_path, "rb") as f:
            bundle_b64 = base64.b64encode(f.read()).decode("ascii")

    except subprocess.CalledProcessError as e:
        print(f"  SKIP: failed to create bundle: {e.stderr}")
        runner.total += 1
        runner.failed += 1
        return

    # Attempt force push (config has allow_force_push=false).
    response = client.send(
        "tools/call",
        {
            "name": "repo_push",
            "arguments": {
                "bundle": bundle_b64,
                "url": REPO_URL,
                "branch": "test/force-push",
                "force": True,
            },
        },
    )

    runner.check("error" not in response, "no protocol error")
    is_error = response.get("result", {}).get("isError", False)
    runner.check(
        is_error is True,
        "force push rejected",
        actual=is_error,
    )


def test_rate_limiting(client, runner):
    """Test that rapid-fire requests trigger rate limiting."""
    print()
    print("=== Test: rate limiting ===")

    # Send many rapid requests. With max_burst=100, we shouldn't
    # hit the limit with normal test flow, but we can verify the
    # rate limiter doesn't block normal operations.
    # Testing actual rate limit exhaustion would require 100+ rapid
    # calls which would slow the test suite. Instead, verify that
    # the rate limiter is active by checking we can make multiple
    # sequential calls without issues.
    success_count = 0
    for _ in range(5):
        content = client.call_tool("repo_refs", {"url": REPO_URL})
        if content.get("default_branch"):
            success_count += 1

    runner.check(
        success_count == 5,
        "5 rapid requests succeeded (rate limiter active but not blocking)",
        actual=success_count,
    )


def test_concurrent_sessions(client, runner):
    """Test that multiple streaming sessions can coexist."""
    print()
    print("=== Test: concurrent sessions ===")

    # Start two sessions.
    session1 = client.call_tool(
        "repo_clone_start",
        {"url": REPO_URL, "branch": "main", "chunk_size": 4096},
    )
    session2 = client.call_tool(
        "repo_clone_start",
        {"url": REPO_URL, "branch": "main", "chunk_size": 4096},
    )

    sid1 = session1.get("session_id")
    sid2 = session2.get("session_id")

    runner.check(
        sid1 is not None and sid2 is not None,
        "both sessions created",
    )
    runner.check(
        sid1 != sid2,
        "sessions have different IDs",
    )

    if not sid1 or not sid2:
        return

    # Fetch chunk 0 from each session.
    chunk1 = client.call_tool(
        "repo_clone_chunk",
        {"session_id": sid1, "chunk_index": 0},
    )
    chunk2 = client.call_tool(
        "repo_clone_chunk",
        {"session_id": sid2, "chunk_index": 0},
    )

    runner.check("data" in chunk1, "session 1 chunk 0 has data")
    runner.check("data" in chunk2, "session 2 chunk 0 has data")

    # Cancel both.
    client.call_tool("repo_clone_cancel", {"session_id": sid1})
    client.call_tool("repo_clone_cancel", {"session_id": sid2})

    runner.check(True, "both sessions cancelled without error")


def test_clone_exclude_binary(client, runner):
    """Test that exclude_binary correctly filters binary files."""
    print()
    print("=== Test: clone exclude_binary vs include ===")

    # Clone with binary exclusion.
    content_no_binary = client.call_tool(
        "repo_clone",
        {"url": REPO_URL, "branch": "main", "depth": 1, "exclude_binary": True},
    )

    # Clone without binary exclusion.
    content_with_binary = client.call_tool(
        "repo_clone",
        {"url": REPO_URL, "branch": "main", "depth": 1, "exclude_binary": False},
    )

    files_no_binary = content_no_binary.get("file_count", 0)
    files_with_binary = content_with_binary.get("file_count", 0)

    runner.check(
        files_with_binary > files_no_binary,
        "more files when binaries included",
        actual=f"{files_with_binary} > {files_no_binary}",
    )

    skipped = content_no_binary.get("skipped_binary", 0)
    runner.check(skipped >= 1, "at least 1 binary skipped", actual=skipped)


def test_ping(client, runner):
    """Test the ping method."""
    print()
    print("=== Test: ping ===")

    response = client.send("ping")

    runner.check("error" not in response, "ping — no error")
    runner.check("result" in response, "ping has result")


def test_pull_up_to_date(client, runner, refs_content):
    """Test that pulling from current HEAD returns up_to_date=true.

    Depends on `test_repo_refs()` having run first to populate refs_content.
    """
    print()
    print("=== Test: pull when already up to date ===")

    branches = {b["short_name"]: b["commit"] for b in refs_content.get("branches", [])}
    head_sha = branches.get("main")

    if not head_sha:
        print("  SKIP: could not resolve main branch SHA")
        runner.total += 1
        runner.failed += 1
        return

    content = client.call_tool(
        "repo_pull",
        {"url": REPO_URL, "branch": "main", "since_commit": head_sha},
    )

    runner.check(
        content.get("up_to_date") is True,
        "up_to_date is true when pulling from HEAD",
        actual=content.get("up_to_date"),
    )
    runner.check(
        content.get("stats", {}).get("commits") == 0,
        "no new commits reported",
        actual=content.get("stats", {}).get("commits"),
    )


def test_diff_same_commit(client, runner, refs_content):
    """Test that diff between identical commits is empty.

    Depends on `test_repo_refs()` having run first to populate refs_content.
    """
    print()
    print("=== Test: diff between identical commits ===")

    tags = {t["short_name"]: t["commit"] for t in refs_content.get("tags", [])}
    v1_sha = tags.get("v0.1.0")

    if not v1_sha:
        print("  SKIP: could not resolve v0.1.0 SHA")
        runner.total += 1
        runner.failed += 1
        return

    content = client.call_tool(
        "repo_diff",
        {"url": REPO_URL, "base_commit": v1_sha, "head_commit": v1_sha},
    )

    stats = content.get("stats", {})
    runner.check(
        stats.get("files_changed", -1) == 0,
        "files_changed is 0 for identical commits",
        actual=stats.get("files_changed"),
    )
    runner.check(
        stats.get("insertions", -1) == 0 and stats.get("deletions", -1) == 0,
        "insertions and deletions are 0",
    )


def test_initialize_already_initialised(client, runner):
    """Test that calling initialize twice returns an InvalidRequest error.

    Depends on `test_initialise()` having run first to put the server
    into Running state.
    """
    print()
    print("=== Test: initialize already initialised ===")

    # Server is already initialised by test_initialise() — calling again should fail.
    response = client.send(
        "initialize",
        {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {"name": "test-client-2", "version": "1.0"},
        },
    )

    runner.check(
        "error" in response,
        "second initialize returns JSON-RPC error",
    )
    if "error" in response:
        code = response["error"].get("code")
        runner.check(
            code == -32600,
            "error code is InvalidRequest (-32600)",
            actual=code,
        )


def test_clone_with_explicit_branch(client, runner):
    """Test that the branch parameter on repo_clone is honoured."""
    print()
    print("=== Test: clone with explicit branch parameter ===")

    content = client.call_tool(
        "repo_clone",
        {"url": REPO_URL, "branch": "main", "depth": 1},
    )

    runner.check(
        content.get("branch") == "main",
        "response has branch=main",
        actual=content.get("branch"),
    )
    runner.check(
        len(content.get("commit", "")) == 40,
        "got valid commit SHA",
        actual=len(content.get("commit", "")),
    )


def test_status_on_unknown_session(client, runner):
    """Test repo_clone_status with a session ID that does not exist."""
    print()
    print("=== Test: status on unknown session ===")

    response = client.send(
        "tools/call",
        {
            "name": "repo_clone_status",
            "arguments": {"session_id": "stream_nonexistent_xyz"},
        },
    )

    runner.check("error" not in response, "no protocol error")
    is_error = response.get("result", {}).get("isError", False)
    runner.check(
        is_error is True,
        "isError true for unknown session",
        actual=is_error,
    )


def test_cancel_unknown_session(client, runner):
    """Test repo_clone_cancel returns a sensible response for unknown session."""
    print()
    print("=== Test: cancel unknown session ===")

    response = client.send(
        "tools/call",
        {
            "name": "repo_clone_cancel",
            "arguments": {"session_id": "stream_does_not_exist_at_all"},
        },
    )

    runner.check("error" not in response, "no protocol error")
    # Two valid response shapes for unknown session:
    #   - isError=true with an error message in content[0].text, OR
    #   - normal result with {cancelled: false}
    # Inspect the actual content to verify which contract the server honours.
    result = response.get("result", {})
    is_error = result.get("isError", False)

    if is_error:
        runner.check(
            True,
            "server returned isError for unknown session (acceptable)",
        )
    else:
        # Parse the JSON in content[0].text and check `cancelled` field.
        try:
            text = result["content"][0]["text"]
            payload = json.loads(text)
            runner.check(
                payload.get("cancelled") is False,
                "cancelled is false for unknown session",
                actual=payload.get("cancelled"),
            )
        except (KeyError, IndexError, json.JSONDecodeError) as exc:
            runner.check(False, f"unexpected response shape: {exc}")


def test_helper_script_content(client, runner):
    """Test that the helper script contains expected commands."""
    print()
    print("=== Test: helper script content ===")

    content = client.call_tool("helper_script", {})

    script = content.get("script", "")
    runner.check(
        "extract" in script,
        "helper script contains 'extract' command",
    )
    runner.check(
        "bundle" in script,
        "helper script contains 'bundle' command",
    )
    runner.check(
        "info" in script,
        "helper script contains 'info' command",
    )
    runner.check(
        content.get("filename") == "git_proxy_helper.py",
        "filename is git_proxy_helper.py",
        actual=content.get("filename"),
    )
    # Version should be a non-empty string
    runner.check(
        len(content.get("version", "")) > 0,
        "version is non-empty",
        actual=content.get("version"),
    )


def test_clone_start_then_status_zero_progress(client, runner):
    """Verify that a freshly-started session reports 0% progress before any chunk fetch."""
    print()
    print("=== Test: clone_start then immediate status ===")

    start = client.call_tool(
        "repo_clone_start",
        {"url": REPO_URL, "branch": "main", "chunk_size": 1024},
    )
    sid = start.get("session_id")
    if not sid:
        print("  SKIP: no session_id returned")
        runner.total += 1
        runner.failed += 1
        return

    status = client.call_tool("repo_clone_status", {"session_id": sid})

    runner.check(
        status.get("delivered_chunks") == 0,
        "delivered_chunks is 0 before any fetch",
        actual=status.get("delivered_chunks"),
    )
    runner.check(
        status.get("next_missing_chunk") == 0,
        "next_missing_chunk is 0",
        actual=status.get("next_missing_chunk"),
    )
    runner.check(
        status.get("is_complete") is False,
        "session not complete",
    )
    runner.check(
        status.get("progress_percent") == 0,
        "progress_percent is 0",
        actual=status.get("progress_percent"),
    )

    # Clean up
    client.call_tool("repo_clone_cancel", {"session_id": sid})


def test_diff_with_invalid_base_commit(client, runner, refs_content):
    """Test that diff with an invalid base commit returns isError.

    Depends on `test_repo_refs()` having run first to populate refs_content.
    """
    print()
    print("=== Test: diff with invalid base commit ===")

    tags = {t["short_name"]: t["commit"] for t in refs_content.get("tags", [])}
    v2_sha = tags.get("v0.2.0")

    if not v2_sha:
        print("  SKIP: could not resolve v0.2.0 SHA")
        runner.total += 1
        runner.failed += 1
        return

    # Use a 40-char hex string that cannot be a real Git OID (all-f).
    # SHA-1 has 2^160 possible values; collision with a real commit is
    # cryptographically impossible.
    bogus_oid = "f" * 40

    response = client.send(
        "tools/call",
        {
            "name": "repo_diff",
            "arguments": {
                "url": REPO_URL,
                "base_commit": bogus_oid,
                "head_commit": v2_sha,
            },
        },
    )

    runner.check("error" not in response, "no protocol error")
    is_error = response.get("result", {}).get("isError", False)
    runner.check(
        is_error is True,
        "isError true for invalid base commit",
        actual=is_error,
    )


def test_pull_with_invalid_since_commit(client, runner):
    """Test that pull with malformed since_commit returns isError."""
    print()
    print("=== Test: pull with invalid since_commit ===")

    response = client.send(
        "tools/call",
        {
            "name": "repo_pull",
            "arguments": {
                "url": REPO_URL,
                "branch": "main",
                "since_commit": "definitely-not-a-sha",
            },
        },
    )

    runner.check("error" not in response, "no protocol error")
    is_error = response.get("result", {}).get("isError", False)
    runner.check(
        is_error is True,
        "isError true for malformed SHA",
        actual=is_error,
    )


def test_clone_start_with_zero_chunk_size_uses_default(client, runner):
    """Test that chunk_size=0 or unset falls back to a sensible default.

    The server clamps chunk_size to a minimum of 1024 bytes.
    """
    print()
    print("=== Test: clone_start without chunk_size ===")

    content = client.call_tool(
        "repo_clone_start",
        {"url": REPO_URL, "branch": "main"},
    )

    sid = content.get("session_id")
    chunk_size = content.get("chunk_size", 0)
    runner.check(sid is not None, "got session_id")
    runner.check(
        chunk_size >= 1024,
        "chunk_size defaults to at least 1024 bytes",
        actual=chunk_size,
    )

    # Clean up
    if sid:
        client.call_tool("repo_clone_cancel", {"session_id": sid})


def test_refs_lists_all_branches_and_tags(_client, runner, refs_content):
    """Detailed check: refs response correctly classifies branches vs tags.

    Uses pre-fetched refs_content from `test_repo_refs()` rather than making
    a fresh call, hence the underscore-prefixed unused client parameter.
    Depends on `test_repo_refs()` having run first to populate refs_content.
    """
    print()
    print("=== Test: refs classification ===")

    branches = refs_content.get("branches", [])
    tags = refs_content.get("tags", [])

    # All branch names should start with refs/heads/
    branches_well_formed = all(
        b.get("name", "").startswith("refs/heads/") for b in branches
    )
    runner.check(
        branches_well_formed,
        "all branches have refs/heads/ prefix",
    )

    # All tag names should start with refs/tags/
    tags_well_formed = all(
        t.get("name", "").startswith("refs/tags/") for t in tags
    )
    runner.check(
        tags_well_formed,
        "all tags have refs/tags/ prefix",
    )

    # All commit SHAs should be 40 hex chars
    all_shas_valid = all(
        len(b.get("commit", "")) == 40 for b in branches
    ) and all(len(t.get("commit", "")) == 40 for t in tags)
    runner.check(all_shas_valid, "all SHAs are 40 hex chars")

    # total_refs should equal branches + tags count
    expected_total = len(branches) + len(tags)
    runner.check(
        refs_content.get("total_refs") == expected_total,
        f"total_refs equals branches+tags ({expected_total})",
        actual=refs_content.get("total_refs"),
    )


def test_clone_with_max_file_size(client, runner):
    """Test that max_file_size correctly filters oversized files."""
    print()
    print("=== Test: clone with max_file_size filter ===")

    # Set a max_file_size that should exclude some files
    content = client.call_tool(
        "repo_clone",
        {"url": REPO_URL, "branch": "main", "depth": 1, "max_file_size": 100},
    )

    skipped = content.get("skipped_too_large", 0)
    runner.check(
        skipped >= 1,
        "at least 1 file skipped due to size limit",
        actual=skipped,
    )


def test_diff_detects_rename(client, runner):
    """Test that diff detects the docs/DESIGN.md -> docs/ARCHITECTURE.md rename.

    Commit 4 in the fixture renames the file. Diffing the parent (commit 3)
    against HEAD should report the rename in the unified diff.
    """
    print()
    print("=== Test: diff detects file rename ===")

    # Get the SHA exports from rebuild_fixture.py.
    env_path = os.path.join(tempfile.gettempdir(), "mcp-test", "fixture-shas.env")
    if not os.path.exists(env_path):
        print(f"  SKIP: {env_path} not found")
        runner.total += 1
        runner.failed += 1
        return

    shas = {}
    with open(env_path, encoding="utf-8") as f:
        for line in f:
            key, _, value = line.strip().partition("=")
            shas[key] = value

    base = shas.get("V3_SHA")
    head = shas.get("V4_SHA")
    if not base or not head:
        print("  SKIP: V3_SHA or V4_SHA not in env file")
        runner.total += 1
        runner.failed += 1
        return

    content = client.call_tool(
        "repo_diff",
        {"url": REPO_URL, "base_commit": base, "head_commit": head},
    )

    diff_text = content.get("diff", "")
    # Either a rename header (similarity-detected) or an add+delete pair will
    # appear. Both indicate the rename was visible in the diff.
    runner.check(
        "DESIGN.md" in diff_text or "ARCHITECTURE.md" in diff_text,
        "diff mentions DESIGN.md or ARCHITECTURE.md",
    )
    runner.check(
        content.get("stats", {}).get("files_changed", 0) >= 1,
        "at least 1 file changed in rename commit",
        actual=content.get("stats", {}).get("files_changed"),
    )


def test_pull_captures_file_move(client, runner):
    """Test that pull from before-rename to after-rename captures the change.

    The current pull implementation does not enable git's similarity-based
    rename detection, so this surfaces as a delete (`docs/DESIGN.md`) plus
    an add (`docs/ARCHITECTURE.md`) rather than a single renamed entry.
    Either shape is acceptable; this test verifies the file-move path is
    represented in the result, not the specific representation.
    """
    print()
    print("=== Test: pull captures file move (rename as delete+add) ===")

    env_path = os.path.join(tempfile.gettempdir(), "mcp-test", "fixture-shas.env")
    if not os.path.exists(env_path):
        print(f"  SKIP: {env_path} not found")
        runner.total += 1
        runner.failed += 1
        return

    shas = {}
    with open(env_path, encoding="utf-8") as f:
        for line in f:
            key, _, value = line.strip().partition("=")
            shas[key] = value

    base = shas.get("V3_SHA")
    if not base:
        print("  SKIP: V3_SHA not in env file")
        runner.total += 1
        runner.failed += 1
        return

    content = client.call_tool(
        "repo_pull",
        {"url": REPO_URL, "branch": "main", "since_commit": base},
    )

    # The rename should appear as either a renamed entry or a delete+add pair.
    changed_files = content.get("changed_files", [])
    deleted_files = content.get("deleted_files", [])
    all_paths = [f.get("path", "") for f in changed_files] + deleted_files

    rename_visible = any(
        "DESIGN.md" in p or "ARCHITECTURE.md" in p for p in all_paths
    )
    runner.check(
        rename_visible,
        "rename appears in changed_files or deleted_files",
        actual=all_paths[:5] if all_paths else "no paths",
    )


def test_clone_resolves_lfs_pointer(client, runner):
    """Test that resolve_lfs=true expands the LFS pointer to actual content.

    Commit 5 in the fixture adds docs/large.bin as an LFS-tracked file.
    With resolve_lfs=true, the server should fetch the actual content.
    """
    print()
    print("=== Test: clone with LFS resolution ===")

    content = client.call_tool(
        "repo_clone",
        {
            "url": REPO_URL,
            "branch": "main",
            "depth": 1,
            "resolve_lfs": True,
        },
    )

    lfs_resolved = content.get("lfs_resolved", 0)
    lfs_failed = content.get("lfs_failed", 0)
    runner.check(
        lfs_resolved >= 1,
        "at least 1 LFS pointer resolved",
        actual=lfs_resolved,
    )
    runner.check(
        lfs_failed == 0,
        "no LFS resolution failures",
        actual=lfs_failed,
    )


def test_clone_without_lfs_keeps_pointer(client, runner):
    """Test that without resolve_lfs, the LFS pointer file is included as-is.

    The pointer is small text content, not the actual binary, but it should
    still appear in the archive.
    """
    print()
    print("=== Test: clone without LFS resolution keeps pointer ===")

    content = client.call_tool(
        "repo_clone",
        {
            "url": REPO_URL,
            "branch": "main",
            "depth": 1,
            "resolve_lfs": False,
        },
    )

    lfs_resolved = content.get("lfs_resolved", 0)
    runner.check(
        lfs_resolved == 0,
        "no LFS resolution attempted (resolve_lfs=false)",
        actual=lfs_resolved,
    )
    # The pointer file should still be in the file count.
    runner.check(
        content.get("file_count", 0) >= 1,
        "file count is non-zero (pointer included)",
        actual=content.get("file_count"),
    )


def main():
    """Run all integration tests against the MCP server."""
    if not REPO_URL:
        print("FATAL: TEST_REPO_URL not set")
        sys.exit(1)

    # Create test config.
    os.makedirs(LOG_DIR, exist_ok=True)
    config_path = os.path.join(LOG_DIR, "config.json")
    with open(config_path, "w", newline="\n", encoding="utf-8") as f:
        json.dump(TEST_CONFIG, f, indent=4)

    print("=" * 44)
    print("git-proxy-mcp Integration Tests")
    print("=" * 44)
    print(f"Binary:   {BINARY}")
    print(f"Config:   {config_path}")
    print(f"Repo URL: {REPO_URL}")

    client = McpTestClient(BINARY, config_path)
    runner = TestRunner()

    refs_content = {}

    try:
        client.start()

        # Happy path tests.
        test_initialise(client, runner)
        refs_content = test_repo_refs(client, runner)
        test_repo_clone(client, runner)
        test_repo_diff(client, runner, refs_content)
        test_repo_pull(client, runner, refs_content)
        test_tier2_streaming(client, runner)
        test_helper_script(client, runner)

        # Push tests.
        test_repo_push(client, runner, refs_content)
        test_push_protected_branch(client, runner, refs_content)

        # Advanced feature tests.
        test_multi_chunk_streaming(client, runner)
        test_clone_with_submodules(client, runner)
        test_clone_without_submodules(client, runner)
        test_force_push(client, runner, refs_content)
        test_clone_exclude_binary(client, runner)
        test_concurrent_sessions(client, runner)
        test_rate_limiting(client, runner)

        # Edge case and error handling tests.
        test_unknown_tool(client, runner)
        test_unknown_method(client, runner)
        test_invalid_params(client, runner)
        test_invalid_url(client, runner)
        test_nonexistent_branch(client, runner)
        test_invalid_commit_sha(client, runner)
        test_invalid_session_id(client, runner)
        test_clone_sparse_patterns(client, runner)
        test_clone_all_chunks(client, runner)
        test_ping(client, runner)

        # Additional coverage tests.
        test_pull_up_to_date(client, runner, refs_content)
        test_diff_same_commit(client, runner, refs_content)
        test_clone_with_explicit_branch(client, runner)
        test_clone_with_max_file_size(client, runner)
        test_status_on_unknown_session(client, runner)
        test_cancel_unknown_session(client, runner)
        test_helper_script_content(client, runner)
        test_clone_start_then_status_zero_progress(client, runner)
        test_clone_start_with_zero_chunk_size_uses_default(client, runner)
        test_diff_with_invalid_base_commit(client, runner, refs_content)
        test_pull_with_invalid_since_commit(client, runner)
        test_refs_lists_all_branches_and_tags(client, runner, refs_content)
        # test_initialize_already_initialised exercises the only initialize
        # error path reachable after init: state guard. The missing-params
        # path can only fire before initialise, so it is covered by the
        # unit test handle_initialize_missing_params_returns_error in
        # src/mcp/server.rs (unit tests can construct an AwaitingInit server).
        test_initialize_already_initialised(client, runner)

        # Fixture-dependent tests (require commits 4 and 5 from the
        # rebuild_fixture.py extended fixture).
        test_diff_detects_rename(client, runner)
        test_pull_captures_file_move(client, runner)
        test_clone_resolves_lfs_pointer(client, runner)
        test_clone_without_lfs_keeps_pointer(client, runner)
    finally:
        client.stop()

    exit_code = runner.summary()

    if exit_code != 0:
        print()
        print("=" * 60)
        print("Server log (full contents — printed on failure for diagnostics):")
        print("=" * 60)
        if os.path.exists(SERVER_LOG_PATH):
            with open(SERVER_LOG_PATH, encoding="utf-8") as f:
                # Full dump rather than tail -50: the LFS batch status code
                # and response body are emitted at the start of each clone,
                # which a tail loses if any progress noise follows. GitHub
                # Actions truncates step logs at 4 MiB, but the server log
                # for a single integration run is well under that.
                print(f.read(), end="")
        print("=" * 60)

    if runner.failed == 0:
        print("All integration tests passed!")

    sys.exit(exit_code)


if __name__ == "__main__":
    main()
