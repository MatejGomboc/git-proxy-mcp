//! Handler for the `helper_script` MCP tool.
//!
//! This tool returns a Python helper script that AI assistants can save and use
//! to simplify working with git-proxy-mcp responses. The script handles:
//!
//! - Parsing nested MCP JSON responses (handles the `[{"type":"text","text":"…"}]` wrapper)
//! - Base64 decoding of archives
//! - Extracting tar.gz archives to directories
//! - Creating git bundles for pushing changes
//! - Inspecting result metadata without extracting
//!
//! # Usage by AI
//!
//! 1. Call `helper_script` tool once per session
//! 2. Save the returned script as `git_proxy_helper.py`
//! 3. Use it for all subsequent operations:
//!    - `python git_proxy_helper.py extract <result.json> [output_dir]`
//!    - `python git_proxy_helper.py bundle <repo_dir> <since_commit> [head_ref] [output_file]`
//!    - `python git_proxy_helper.py info <result.json>`

use serde::Serialize;

/// Result of the `helper_script` tool.
#[derive(Debug, Clone, Serialize)]
pub struct HelperScriptResult {
    /// The Python helper script content
    pub script: String,

    /// Suggested filename to save the script as
    pub filename: String,

    /// Brief usage instructions
    pub usage: String,

    /// Version of the helper script
    pub version: String,
}

/// The Python helper script content.
///
/// This script is designed to work with Python 3.6+ using only stdlib modules.
const HELPER_SCRIPT: &str = r#"#!/usr/bin/env python3
"""
git-proxy-mcp helper script for AI assistants.

This script simplifies working with git-proxy-mcp tool responses by handling
JSON parsing, base64 decoding, and archive extraction automatically.

Usage:
    python git_proxy_helper.py extract <result_json> [output_dir]
    python git_proxy_helper.py bundle <repo_dir> <since_commit> [head_ref] [output_file]
    python git_proxy_helper.py info <result_json>

Examples:
    # Extract a repo_clone result
    python git_proxy_helper.py extract clone_result.json ./my-repo

    # Extract from external storage path
    python git_proxy_helper.py extract /mnt/user-data/tool_results/result.json ./repo

    # Create a bundle for repo_push
    python git_proxy_helper.py bundle ./my-repo abc123def HEAD

    # Show info about a result without extracting
    python git_proxy_helper.py info clone_result.json

Version: 1.1.0
"""

import json
import base64
import tarfile
import io
import sys
import os
import subprocess
from pathlib import Path


def parse_mcp_result(json_path: str) -> dict:
    """
    Parse an MCP tool result JSON file.

    Handles both direct JSON and the MCP wrapper format:
    [{"type": "text", "text": "{...}"}]
    """
    with open(json_path, 'r', encoding='utf-8') as f:
        data = json.load(f)

    # Handle MCP content wrapper
    if isinstance(data, list) and data and isinstance(data[0], dict):
        if 'text' in data[0]:
            data = json.loads(data[0]['text'])

    return data


def extract_archive(json_path: str, output_dir: str = ".") -> dict:
    """
    Extract a repo_clone or repo_pull archive result.

    Args:
        json_path: Path to the JSON result file
        output_dir: Directory to extract files to (created if needed)

    Returns:
        Dict with extraction metadata (commit, branch, file_count, etc.)
    """
    data = parse_mcp_result(json_path)

    # Get the archive field (repo_clone uses 'archive', repo_pull uses 'files_archive')
    archive_b64 = data.get('archive') or data.get('files_archive')

    if not archive_b64:
        raise ValueError("No archive found in result. Is this a clone/pull result?")

    # Create output directory
    output_path = Path(output_dir)
    output_path.mkdir(parents=True, exist_ok=True)

    # Decode and extract
    archive_bytes = base64.b64decode(archive_b64)

    with tarfile.open(fileobj=io.BytesIO(archive_bytes), mode='r:gz') as tar:
        tar.extractall(output_path)

    # Build result info
    result = {
        'extracted_to': str(output_path.absolute()),
        'commit': data.get('commit', data.get('new_commit', 'unknown')),
        'branch': data.get('branch', 'unknown'),
        'file_count': data.get('file_count', len(list(output_path.rglob('*')))),
    }

    # Add optional fields if present
    if 'skipped_by_filter' in data and data['skipped_by_filter'] > 0:
        result['skipped_by_filter'] = data['skipped_by_filter']
    if 'skipped_binary' in data and data['skipped_binary'] > 0:
        result['skipped_binary'] = data['skipped_binary']
    if 'deleted_files' in data:
        result['deleted_files'] = data['deleted_files']

    return result


def create_bundle(repo_dir: str, since_commit: str, head_ref: str = "HEAD",
                  output_file: str = None) -> dict:
    """
    Create a git bundle for use with repo_push.

    Args:
        repo_dir: Path to the git repository
        since_commit: Base commit (what the remote has)
        head_ref: What to include (default: HEAD)
        output_file: Output bundle file (default: auto-generated)

    Returns:
        Dict with bundle path and base64-encoded content
    """
    repo_path = Path(repo_dir).absolute()

    if not (repo_path / '.git').exists() and not (repo_path / 'HEAD').exists():
        raise ValueError(f"Not a git repository: {repo_path}")

    # Generate output filename if not provided
    if output_file is None:
        output_file = f"bundle_{since_commit[:8]}_{head_ref.replace('/', '_')}.bundle"

    output_path = Path(output_file).absolute()

    # Create the bundle
    # Format: git bundle create <file> <since>..<head>
    bundle_range = f"{since_commit}..{head_ref}"

    result = subprocess.run(
        ['git', 'bundle', 'create', str(output_path), bundle_range],
        cwd=repo_path,
        capture_output=True,
        text=True
    )

    if result.returncode != 0:
        raise RuntimeError(f"git bundle failed: {result.stderr}")

    # Read and encode the bundle
    with open(output_path, 'rb') as f:
        bundle_bytes = f.read()

    bundle_b64 = base64.b64encode(bundle_bytes).decode('ascii')

    return {
        'bundle_path': str(output_path),
        'bundle_base64': bundle_b64,
        'bundle_size': len(bundle_bytes),
        'range': bundle_range,
    }


def show_info(json_path: str) -> dict:
    """
    Show information about a result without extracting.

    Args:
        json_path: Path to the JSON result file

    Returns:
        Dict with result metadata
    """
    data = parse_mcp_result(json_path)

    info = {}

    # Common fields. `base_commit`/`new_commit` are repo_pull,
    # `base_commit`/`head_commit` are repo_diff, `commit` is repo_clone /
    # repo_clone_start / repo_push, and the skipped/lfs/submodule counters
    # appear on repo_clone and repo_clone_start.
    for key in ['commit', 'branch', 'file_count', 'archive_size',
                'base_commit', 'new_commit', 'head_commit', 'up_to_date',
                'skipped_by_filter', 'skipped_binary', 'skipped_too_large',
                'lfs_resolved', 'lfs_failed',
                'submodules_included', 'submodules_failed',
                'deleted_files']:
        if key in data:
            info[key] = data[key]

    # Check archive presence
    if 'archive' in data:
        info['has_archive'] = True
        info['archive_b64_length'] = len(data['archive'])
    if 'files_archive' in data:
        info['has_files_archive'] = True
        info['files_archive_b64_length'] = len(data['files_archive'])
    if 'diff' in data:
        info['has_diff'] = True
        info['diff_lines'] = data['diff'].count('\n')

    return info


def main():
    if len(sys.argv) < 2:
        print(__doc__)
        sys.exit(1)

    command = sys.argv[1].lower()

    try:
        if command == 'extract':
            if len(sys.argv) < 3:
                print("Usage: git_proxy_helper.py extract <result.json> [output_dir]")
                sys.exit(1)

            json_path = sys.argv[2]
            output_dir = sys.argv[3] if len(sys.argv) > 3 else "."

            result = extract_archive(json_path, output_dir)

            print(f"Extracted to: {result['extracted_to']}")
            print(f"Commit: {result['commit']}")
            print(f"Branch: {result['branch']}")
            print(f"Files: {result['file_count']}")

            if 'deleted_files' in result and result['deleted_files']:
                print(f"Deleted files: {', '.join(result['deleted_files'])}")

        elif command == 'bundle':
            if len(sys.argv) < 4:
                print("Usage: git_proxy_helper.py bundle <repo_dir> <since_commit> [head_ref] [output_file]")
                sys.exit(1)

            repo_dir = sys.argv[2]
            since_commit = sys.argv[3]
            head_ref = sys.argv[4] if len(sys.argv) > 4 else "HEAD"
            output_file = sys.argv[5] if len(sys.argv) > 5 else None

            result = create_bundle(repo_dir, since_commit, head_ref, output_file)

            print(f"Bundle created: {result['bundle_path']}")
            print(f"Size: {result['bundle_size']} bytes")
            print(f"Range: {result['range']}")
            print(f"\nBase64 for repo_push (first 100 chars):")
            print(f"{result['bundle_base64'][:100]}...")

        elif command == 'info':
            if len(sys.argv) < 3:
                print("Usage: git_proxy_helper.py info <result.json>")
                sys.exit(1)

            json_path = sys.argv[2]
            info = show_info(json_path)

            print("Result info:")
            for key, value in info.items():
                print(f"  {key}: {value}")

        else:
            print(f"Unknown command: {command}")
            print("Available commands: extract, bundle, info")
            sys.exit(1)

    except FileNotFoundError as e:
        print(f"Error: File not found - {e.filename}")
        sys.exit(1)
    except json.JSONDecodeError as e:
        print(f"Error: Invalid JSON - {e}")
        sys.exit(1)
    except Exception as e:
        print(f"Error: {e}")
        sys.exit(1)


if __name__ == '__main__':
    main()
"#;

/// Handle the `helper_script` tool call.
///
/// This tool returns a Python helper script that simplifies working with
/// git-proxy-mcp responses. The AI should save this script once per session
/// and use it for all subsequent clone/pull/push operations.
///
/// # Returns
///
/// A `HelperScriptResult` containing:
/// - The Python script content
/// - Suggested filename
/// - Usage instructions
#[must_use]
pub fn handle_helper_script() -> HelperScriptResult {
    HelperScriptResult {
        script: HELPER_SCRIPT.to_string(),
        filename: "git_proxy_helper.py".to_string(),
        usage: "Save script and use:\n  \
            python git_proxy_helper.py extract <result.json> [output_dir]   # Extract clone/pull\n  \
            python git_proxy_helper.py bundle <repo_dir> <since_commit>     # Create push bundle\n  \
            python git_proxy_helper.py info <result.json>                   # Show result info"
            .to_string(),
        version: "1.1.0".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helper_script_result_serializes() {
        let result = handle_helper_script();
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"script\":"));
        assert!(json.contains("\"filename\":\"git_proxy_helper.py\""));
        assert!(json.contains("\"version\":\"1.1.0\""));
    }

    #[test]
    fn helper_script_contains_key_functions() {
        let result = handle_helper_script();
        assert!(result.script.contains("def extract_archive"));
        assert!(result.script.contains("def create_bundle"));
        assert!(result.script.contains("def parse_mcp_result"));
        assert!(result.script.contains("def show_info"));
    }

    #[test]
    fn helper_script_handles_mcp_wrapper() {
        let result = handle_helper_script();
        // Verify the script handles the MCP wrapper format
        assert!(result.script.contains("if 'text' in data[0]"));
        assert!(result.script.contains("json.loads(data[0]['text'])"));
    }

    #[test]
    fn helper_script_uses_correct_repo_pull_archive_field() {
        // Regression guard: the script must look up `files_archive`
        // (the actual `RepoPullResult` field name) and not the historical
        // typo `changed_files_archive` it used to look for.
        let result = handle_helper_script();
        assert!(
            result.script.contains("data.get('files_archive')"),
            "script must look up the actual repo_pull archive field name"
        );
        assert!(
            !result.script.contains("changed_files_archive"),
            "script must not reference the obsolete `changed_files_archive` key"
        );
    }

    #[test]
    fn helper_script_show_info_uses_real_commit_field_names() {
        // Regression guard: `show_info` used to enumerate `old_commit`,
        // a key no MCP tool has ever returned. The real commit fields
        // are `commit` (repo_clone / repo_clone_start / repo_push),
        // `base_commit` + `new_commit` (repo_pull), and
        // `base_commit` + `head_commit` (repo_diff).
        let result = handle_helper_script();
        assert!(
            !result.script.contains("'old_commit'"),
            "script must not reference the non-existent `old_commit` key"
        );
        assert!(
            result.script.contains("'base_commit'"),
            "script must enumerate `base_commit` (repo_pull / repo_diff)"
        );
        assert!(
            result.script.contains("'head_commit'"),
            "script must enumerate `head_commit` (repo_diff)"
        );
    }
}
