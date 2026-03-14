#!/usr/bin/env python3
"""Integration tests for git-proxy-mcp MCP server.

Spawns the server, sends JSON-RPC requests over stdin/stdout,
and validates responses against the test fixture repository.

Requires:
    - target/release/git-proxy-mcp (built beforehand)
    - TEST_REPO_URL environment variable (private test fixture repo)
    - git credentials configured (for private repo access)

Test fixture repo (git-proxy-mcp-test-dummy) has:
    - 2 commits, 2 tags (v0.1.0, v0.2.0)
    - 5 files: README.md, src/main.rs, src/lib.rs, docs/DESIGN.md, docs/pixel.png
    - v0.1.0 -> v0.2.0 diff: 2 files changed (src/lib.rs, docs/DESIGN.md)
"""

import base64
import json
import os
import select
import shutil
import subprocess
import sys
import tempfile
import time

BINARY = "./target/release/git-proxy-mcp"
REPO_URL = os.environ.get("TEST_REPO_URL", "")
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
        files_changed == 2,
        "files changed",
        actual=files_changed,
        expected=2,
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

        # Create a git bundle containing the new commit.
        # Use main..HEAD so the bundle contains only the new commit
        # with main as the prerequisite.
        bundle_path = os.path.join(work_dir, "push.bundle")
        subprocess.run(
            ["git", "bundle", "create", bundle_path, "main..HEAD",
             "--branches", "test/integration-push"],
            cwd=repo_dir, capture_output=True, text=True, check=True,
        )

        # Verify bundle was created and read it.
        with open(bundle_path, "rb") as f:
            bundle_bytes = f.read()
        print(f"  Bundle size: {len(bundle_bytes)} bytes")
        print(f"  Bundle header: {bundle_bytes[:30]}")
        bundle_b64 = base64.b64encode(bundle_bytes).decode("ascii")

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


def test_ping(client, runner):
    """Test the ping method."""
    print()
    print("=== Test: ping ===")

    response = client.send("ping")

    runner.check("error" not in response, "ping — no error")
    runner.check("result" in response, "ping has result")


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
    finally:
        client.stop()

    exit_code = runner.summary()

    if exit_code != 0:
        print()
        print("Server log (last 50 lines):")
        if os.path.exists(SERVER_LOG_PATH):
            with open(SERVER_LOG_PATH, encoding="utf-8") as f:
                lines = f.readlines()
                for line in lines[-50:]:
                    print(line, end="")

    if runner.failed == 0:
        print("All integration tests passed!")

    sys.exit(exit_code)


if __name__ == "__main__":
    main()
