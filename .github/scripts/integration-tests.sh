#!/usr/bin/env bash
# Integration tests for git-proxy-mcp MCP server.
# Spawns the server, sends JSON-RPC requests, and validates responses.
#
# Requires:
#   - target/release/git-proxy-mcp (built beforehand)
#   - /tmp/mcp-test/config.json (test configuration)
#   - TEST_REPO_URL env var (private test fixture repo)
#   - git credentials configured (for private repo access)
#
# Test fixture repo (git-proxy-mcp-test-dummy) has:
#   - 2 commits, 2 tags (v0.1.0, v0.2.0)
#   - 5 files: README.md, src/main.rs, src/lib.rs, docs/DESIGN.md, docs/pixel.png (binary)
#   - v0.1.0 → v0.2.0 diff: 2 files changed (src/lib.rs, docs/DESIGN.md)

set -euo pipefail

BINARY="./target/release/git-proxy-mcp"
CONFIG="/tmp/mcp-test/config.json"
FIFO_IN="/tmp/mcp-test/stdin.fifo"
FIFO_OUT="/tmp/mcp-test/stdout.fifo"
PASSED=0
FAILED=0
TOTAL=0

# --- Helpers ---

cleanup() {
    if [[ -n "${SERVER_PID:-}" ]] && kill -0 "$SERVER_PID" 2>/dev/null; then
        kill "$SERVER_PID" 2>/dev/null || true
        wait "$SERVER_PID" 2>/dev/null || true
    fi
    rm -f "$FIFO_IN" "$FIFO_OUT"
}
trap cleanup EXIT

start_server() {
    rm -f "$FIFO_IN" "$FIFO_OUT"
    mkfifo "$FIFO_IN" "$FIFO_OUT"

    "$BINARY" --config "$CONFIG" < "$FIFO_IN" > "$FIFO_OUT" 2>/tmp/mcp-test/server.log &
    SERVER_PID=$!

    # Open the write end of the input FIFO (keeps it open for multiple writes)
    exec 3>"$FIFO_IN"

    # Open the read end of the output FIFO
    exec 4<"$FIFO_OUT"

    sleep 1

    if ! kill -0 "$SERVER_PID" 2>/dev/null; then
        echo "FATAL: Server failed to start"
        cat /tmp/mcp-test/server.log
        exit 1
    fi

    echo "Server started (PID $SERVER_PID)"
}

stop_server() {
    # Close the write end — server sees EOF on stdin and shuts down
    exec 3>&-
    wait "$SERVER_PID" 2>/dev/null || true
    exec 4<&-
    SERVER_PID=""
}

# Send a JSON-RPC request and read the response.
# Usage: response=$(send_request '{"jsonrpc":"2.0","id":1,...}')
send_request() {
    local request="$1"
    echo "$request" >&3

    # Read one line of response (JSON-RPC uses newline-delimited JSON)
    local response
    if ! read -r -t 60 response <&4; then
        echo '{"error":"timeout reading response"}'
        return 1
    fi
    echo "$response"
}

# Assert that a JSON response contains an expected value at a given jq path.
# Usage: assert_json "$response" '.result.branches | length' '1' 'has one branch'
assert_json() {
    local response="$1"
    local jq_path="$2"
    local expected="$3"
    local description="$4"

    TOTAL=$((TOTAL + 1))

    local actual
    actual=$(echo "$response" | jq -r "$jq_path" 2>/dev/null)

    if [[ "$actual" == "$expected" ]]; then
        echo "  PASS: $description (got $actual)"
        PASSED=$((PASSED + 1))
    else
        echo "  FAIL: $description (expected $expected, got $actual)"
        FAILED=$((FAILED + 1))
    fi
}

# Assert that a JSON response value is greater than a threshold.
# Usage: assert_json_gt "$response" '.result.size' '0' 'size is positive'
assert_json_gt() {
    local response="$1"
    local jq_path="$2"
    local threshold="$3"
    local description="$4"

    TOTAL=$((TOTAL + 1))

    local actual
    actual=$(echo "$response" | jq -r "$jq_path" 2>/dev/null)

    if [[ "$actual" -gt "$threshold" ]] 2>/dev/null; then
        echo "  PASS: $description (got $actual > $threshold)"
        PASSED=$((PASSED + 1))
    else
        echo "  FAIL: $description (expected > $threshold, got $actual)"
        FAILED=$((FAILED + 1))
    fi
}

# Assert that the response has no error field.
# Usage: assert_no_error "$response" 'tool name'
assert_no_error() {
    local response="$1"
    local description="$2"

    TOTAL=$((TOTAL + 1))

    local has_error
    has_error=$(echo "$response" | jq 'has("error")' 2>/dev/null)

    if [[ "$has_error" == "false" ]]; then
        echo "  PASS: $description — no error"
        PASSED=$((PASSED + 1))
    else
        local error_msg
        error_msg=$(echo "$response" | jq -r '.error.message // .error' 2>/dev/null)
        echo "  FAIL: $description — error: $error_msg"
        FAILED=$((FAILED + 1))
    fi
}

# --- Tests ---

test_initialize() {
    echo ""
    echo "=== Test: MCP Initialise ==="

    local response
    response=$(send_request '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"integration-test","version":"1.0.0"}}}')

    assert_no_error "$response" "initialise"
    assert_json "$response" '.result.protocolVersion' '2024-11-05' 'protocol version'
    assert_json "$response" '.result.serverInfo.name' 'git-proxy-mcp' 'server name'

    # Send initialized notification (no response expected)
    echo '{"jsonrpc":"2.0","method":"notifications/initialized"}' >&3
    sleep 0.5
}

test_repo_refs() {
    echo ""
    echo "=== Test: repo_refs ==="

    local response
    response=$(send_request "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"repo_refs\",\"arguments\":{\"url\":\"$TEST_REPO_URL\"}}}")

    assert_no_error "$response" "repo_refs"

    # Parse the text content (tool results are wrapped in content array)
    local content
    content=$(echo "$response" | jq -r '.result.content[0].text' 2>/dev/null)

    local branch_count
    branch_count=$(echo "$content" | jq '.branches | length' 2>/dev/null)
    local tag_count
    tag_count=$(echo "$content" | jq '.tags | length' 2>/dev/null)
    local default_branch
    default_branch=$(echo "$content" | jq -r '.default_branch' 2>/dev/null)

    TOTAL=$((TOTAL + 1))
    if [[ "$branch_count" -ge 1 ]]; then
        echo "  PASS: has branches (got $branch_count)"
        PASSED=$((PASSED + 1))
    else
        echo "  FAIL: expected branches (got $branch_count)"
        FAILED=$((FAILED + 1))
    fi

    TOTAL=$((TOTAL + 1))
    if [[ "$tag_count" -ge 2 ]]; then
        echo "  PASS: has tags (got $tag_count)"
        PASSED=$((PASSED + 1))
    else
        echo "  FAIL: expected >= 2 tags (got $tag_count)"
        FAILED=$((FAILED + 1))
    fi

    TOTAL=$((TOTAL + 1))
    if [[ "$default_branch" == "main" ]]; then
        echo "  PASS: default branch is main"
        PASSED=$((PASSED + 1))
    else
        echo "  FAIL: expected default branch 'main', got '$default_branch'"
        FAILED=$((FAILED + 1))
    fi
}

test_repo_clone() {
    echo ""
    echo "=== Test: repo_clone (Tier 1) ==="

    local response
    response=$(send_request "{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"tools/call\",\"params\":{\"name\":\"repo_clone\",\"arguments\":{\"url\":\"$TEST_REPO_URL\",\"branch\":\"main\",\"depth\":1,\"exclude_binary\":true}}}")

    assert_no_error "$response" "repo_clone"

    local content
    content=$(echo "$response" | jq -r '.result.content[0].text' 2>/dev/null)

    # Should have files and an archive
    local file_count
    file_count=$(echo "$content" | jq '.file_count' 2>/dev/null)

    TOTAL=$((TOTAL + 1))
    if [[ "$file_count" -ge 4 ]]; then
        echo "  PASS: file count (got $file_count)"
        PASSED=$((PASSED + 1))
    else
        echo "  FAIL: expected >= 4 files (got $file_count)"
        FAILED=$((FAILED + 1))
    fi

    # Binary should be skipped
    local binary_skipped
    binary_skipped=$(echo "$content" | jq '.binary_files_skipped // 0' 2>/dev/null)

    TOTAL=$((TOTAL + 1))
    if [[ "$binary_skipped" -ge 1 ]]; then
        echo "  PASS: binary files skipped (got $binary_skipped)"
        PASSED=$((PASSED + 1))
    else
        echo "  FAIL: expected >= 1 binary skipped (got $binary_skipped)"
        FAILED=$((FAILED + 1))
    fi
}

test_repo_diff() {
    echo ""
    echo "=== Test: repo_diff ==="

    # Get tag SHAs from refs first
    local refs_response
    refs_response=$(send_request "{\"jsonrpc\":\"2.0\",\"id\":4,\"method\":\"tools/call\",\"params\":{\"name\":\"repo_refs\",\"arguments\":{\"url\":\"$TEST_REPO_URL\"}}}")

    local refs_content
    refs_content=$(echo "$refs_response" | jq -r '.result.content[0].text' 2>/dev/null)

    local v1_sha
    v1_sha=$(echo "$refs_content" | jq -r '.tags[] | select(.name == "v0.1.0") | .commit' 2>/dev/null)
    local v2_sha
    v2_sha=$(echo "$refs_content" | jq -r '.tags[] | select(.name == "v0.2.0") | .commit' 2>/dev/null)

    if [[ -z "$v1_sha" || -z "$v2_sha" ]]; then
        echo "  SKIP: could not resolve tag SHAs (v1=$v1_sha, v2=$v2_sha)"
        TOTAL=$((TOTAL + 1))
        FAILED=$((FAILED + 1))
        return
    fi

    local response
    response=$(send_request "{\"jsonrpc\":\"2.0\",\"id\":5,\"method\":\"tools/call\",\"params\":{\"name\":\"repo_diff\",\"arguments\":{\"url\":\"$TEST_REPO_URL\",\"base_commit\":\"$v1_sha\",\"head_commit\":\"$v2_sha\"}}}")

    assert_no_error "$response" "repo_diff"

    local content
    content=$(echo "$response" | jq -r '.result.content[0].text' 2>/dev/null)

    local files_changed
    files_changed=$(echo "$content" | jq '.stats.files_changed' 2>/dev/null)

    TOTAL=$((TOTAL + 1))
    if [[ "$files_changed" == "2" ]]; then
        echo "  PASS: files changed (got $files_changed)"
        PASSED=$((PASSED + 1))
    else
        echo "  FAIL: expected 2 files changed (got $files_changed)"
        FAILED=$((FAILED + 1))
    fi
}

test_repo_pull() {
    echo ""
    echo "=== Test: repo_pull ==="

    # Use v0.1.0 as since_commit to pull changes up to HEAD
    local refs_response
    refs_response=$(send_request "{\"jsonrpc\":\"2.0\",\"id\":6,\"method\":\"tools/call\",\"params\":{\"name\":\"repo_refs\",\"arguments\":{\"url\":\"$TEST_REPO_URL\"}}}")

    local refs_content
    refs_content=$(echo "$refs_response" | jq -r '.result.content[0].text' 2>/dev/null)

    local v1_sha
    v1_sha=$(echo "$refs_content" | jq -r '.tags[] | select(.name == "v0.1.0") | .commit' 2>/dev/null)

    if [[ -z "$v1_sha" ]]; then
        echo "  SKIP: could not resolve v0.1.0 SHA"
        TOTAL=$((TOTAL + 1))
        FAILED=$((FAILED + 1))
        return
    fi

    local response
    response=$(send_request "{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"tools/call\",\"params\":{\"name\":\"repo_pull\",\"arguments\":{\"url\":\"$TEST_REPO_URL\",\"branch\":\"main\",\"since_commit\":\"$v1_sha\"}}}")

    assert_no_error "$response" "repo_pull"

    local content
    content=$(echo "$response" | jq -r '.result.content[0].text' 2>/dev/null)

    local commit_count
    commit_count=$(echo "$content" | jq '.commits | length' 2>/dev/null)

    TOTAL=$((TOTAL + 1))
    if [[ "$commit_count" -ge 1 ]]; then
        echo "  PASS: got commits (count: $commit_count)"
        PASSED=$((PASSED + 1))
    else
        echo "  FAIL: expected >= 1 commits (got $commit_count)"
        FAILED=$((FAILED + 1))
    fi
}

test_tier2_streaming() {
    echo ""
    echo "=== Test: Tier 2 streaming (clone_start → status → chunk → cancel) ==="

    # Start chunked clone
    local start_response
    start_response=$(send_request "{\"jsonrpc\":\"2.0\",\"id\":8,\"method\":\"tools/call\",\"params\":{\"name\":\"repo_clone_start\",\"arguments\":{\"url\":\"$TEST_REPO_URL\",\"branch\":\"main\",\"chunk_size\":32768}}}")

    assert_no_error "$start_response" "repo_clone_start"

    local start_content
    start_content=$(echo "$start_response" | jq -r '.result.content[0].text' 2>/dev/null)

    local session_id
    session_id=$(echo "$start_content" | jq -r '.session_id' 2>/dev/null)
    local total_chunks
    total_chunks=$(echo "$start_content" | jq '.total_chunks' 2>/dev/null)

    TOTAL=$((TOTAL + 1))
    if [[ -n "$session_id" && "$session_id" != "null" ]]; then
        echo "  PASS: got session_id ($session_id)"
        PASSED=$((PASSED + 1))
    else
        echo "  FAIL: no session_id"
        FAILED=$((FAILED + 1))
        return
    fi

    TOTAL=$((TOTAL + 1))
    if [[ "$total_chunks" -ge 1 ]]; then
        echo "  PASS: total chunks ($total_chunks)"
        PASSED=$((PASSED + 1))
    else
        echo "  FAIL: expected >= 1 chunks (got $total_chunks)"
        FAILED=$((FAILED + 1))
    fi

    # Check status (should be 0% before fetching any chunks)
    local status_response
    status_response=$(send_request "{\"jsonrpc\":\"2.0\",\"id\":9,\"method\":\"tools/call\",\"params\":{\"name\":\"repo_clone_status\",\"arguments\":{\"session_id\":\"$session_id\"}}}")

    assert_no_error "$status_response" "repo_clone_status"

    local status_content
    status_content=$(echo "$status_response" | jq -r '.result.content[0].text' 2>/dev/null)

    local delivered
    delivered=$(echo "$status_content" | jq '.delivered_chunks' 2>/dev/null)

    TOTAL=$((TOTAL + 1))
    if [[ "$delivered" == "0" ]]; then
        echo "  PASS: 0 chunks delivered before fetch"
        PASSED=$((PASSED + 1))
    else
        echo "  FAIL: expected 0 delivered (got $delivered)"
        FAILED=$((FAILED + 1))
    fi

    # Fetch chunk 0
    local chunk_response
    chunk_response=$(send_request "{\"jsonrpc\":\"2.0\",\"id\":10,\"method\":\"tools/call\",\"params\":{\"name\":\"repo_clone_chunk\",\"arguments\":{\"session_id\":\"$session_id\",\"chunk_index\":0}}}")

    assert_no_error "$chunk_response" "repo_clone_chunk(0)"

    local chunk_content
    chunk_content=$(echo "$chunk_response" | jq -r '.result.content[0].text' 2>/dev/null)

    local has_data
    has_data=$(echo "$chunk_content" | jq 'has("data")' 2>/dev/null)

    TOTAL=$((TOTAL + 1))
    if [[ "$has_data" == "true" ]]; then
        echo "  PASS: chunk 0 has data"
        PASSED=$((PASSED + 1))
    else
        echo "  FAIL: chunk 0 missing data"
        FAILED=$((FAILED + 1))
    fi

    # Cancel session
    local cancel_response
    cancel_response=$(send_request "{\"jsonrpc\":\"2.0\",\"id\":11,\"method\":\"tools/call\",\"params\":{\"name\":\"repo_clone_cancel\",\"arguments\":{\"session_id\":\"$session_id\"}}}")

    assert_no_error "$cancel_response" "repo_clone_cancel"

    local cancel_content
    cancel_content=$(echo "$cancel_response" | jq -r '.result.content[0].text' 2>/dev/null)

    local cancelled
    cancelled=$(echo "$cancel_content" | jq '.cancelled' 2>/dev/null)

    TOTAL=$((TOTAL + 1))
    if [[ "$cancelled" == "true" ]]; then
        echo "  PASS: session cancelled"
        PASSED=$((PASSED + 1))
    else
        echo "  FAIL: cancel returned cancelled=$cancelled"
        FAILED=$((FAILED + 1))
    fi
}

test_helper_script() {
    echo ""
    echo "=== Test: helper_script ==="

    local response
    response=$(send_request '{"jsonrpc":"2.0","id":12,"method":"tools/call","params":{"name":"helper_script","arguments":{}}}')

    assert_no_error "$response" "helper_script"

    local content
    content=$(echo "$response" | jq -r '.result.content[0].text' 2>/dev/null)

    TOTAL=$((TOTAL + 1))
    if echo "$content" | grep -q "extract"; then
        echo "  PASS: helper script contains 'extract'"
        PASSED=$((PASSED + 1))
    else
        echo "  FAIL: helper script missing 'extract'"
        FAILED=$((FAILED + 1))
    fi
}

# --- Main ---

echo "============================================"
echo "git-proxy-mcp Integration Tests"
echo "============================================"
echo "Binary:   $BINARY"
echo "Config:   $CONFIG"
echo "Repo URL: ${TEST_REPO_URL:-(not set)}"
echo ""

if [[ -z "${TEST_REPO_URL:-}" ]]; then
    echo "FATAL: TEST_REPO_URL not set"
    exit 1
fi

start_server

test_initialize
test_repo_refs
test_repo_clone
test_repo_diff
test_repo_pull
test_tier2_streaming
test_helper_script

stop_server

echo ""
echo "============================================"
echo "Results: $PASSED passed, $FAILED failed, $TOTAL total"
echo "============================================"

if [[ "$FAILED" -gt 0 ]]; then
    echo ""
    echo "Server log (last 50 lines):"
    tail -50 /tmp/mcp-test/server.log
    exit 1
fi

echo "All integration tests passed!"
