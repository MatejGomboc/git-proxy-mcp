# git-proxy-mcp

**Your Git credentials stay on your machine. Always.**

A secure MCP server that lets AI assistants work with Git repositories without ever seeing your credentials.

---

## Who Is This For?

- **Security-conscious developers** who want explicit control over credential flow
- **Enterprise/regulated environments** where compliance requires credentials to stay in-house
- **Self-hosters** already running local AI tooling who want the same control for Git
- **Anyone** who prefers "trust but verify" over "just trust"

---

## The Problem

**Credential exposure:** When AI coding assistants access your Git repositories, your credentials (PATs, SSH keys,
tokens) flow through layers you don't fully control. Even with trusted providers, that's a wider attack surface than
necessary.

**Workflow friction:** Existing solutions like GitHub's MCP server require AI assistants to work with files through
API calls. But AI assistants work better when they can transport repository files to their own environment, work on
them there, and push changes back. That's the natural Git workflow — clone, edit, commit, push — not file-by-file API
manipulation.

## The Solution

git-proxy-mcp acts as a local proxy between your AI assistant and Git hosting services. It:

- **Keeps credentials local** — Your PATs and SSH keys never leave your machine
- **Exposes only data** — AI assistants receive repository content (files, commits, branches) but never authentication
  secrets
- **Enables native Git workflow** — Clone, edit, commit, push. AI assistants work with full repo copies, not API calls
- **Runs locally** — stdio transport means no network exposure between the MCP server and client

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                              User's PC                                      │
│                                                                             │
│   ┌─────────────────────────────────────────────────────────────────────┐  │
│   │                     git-proxy-mcp                                    │  │
│   │                                                                      │  │
│   │   config.json ──┐                                                    │  │
│   │   (PAT, keys)   │  NEVER                                             │  │
│   │                 │  LEAVES ──────────────────────┐                    │  │
│   │                 ▼  HERE                         │                    │  │
│   │          ┌─────────────┐                        │                    │  │
│   │          │ Auth Module │                        │                    │  │
│   │          │ (internal)  │                        │                    │  │
│   │          └──────┬──────┘                        │                    │  │
│   │                 │                               │                    │  │
│   │                 │ HTTPS + PAT                   │                    │  │
│   │                 ▼                               │                    │  │
│   │          ┌─────────────┐                        │                    │  │
│   │          │   GitHub    │                        │                    │  │
│   │          │   GitLab    │                        │                    │  │
│   │          └──────┬──────┘                        │                    │  │
│   │                 │                               │                    │  │
│   │                 │ Git pack data                 │                    │  │
│   │                 │ (files, commits)              │                    │  │
│   │                 │ NO CREDENTIALS                │                    │  │
│   │                 ▼                               │                    │  │
│   │          ┌─────────────┐                        │                    │  │
│   │          │ MCP Response│ ◄──────────────────────┘                    │  │
│   │          │ (data only) │                                             │  │
│   │          └──────┬──────┘                                             │  │
│   │                 │                                                    │  │
│   └─────────────────┼────────────────────────────────────────────────────┘  │
│                     │ stdio (local process, no network)                     │
│                     ▼                                                       │
│              ┌─────────────┐                                                │
│              │Claude Desktop│                                               │
│              │ / MCP Client │                                               │
│              └──────┬──────┘                                                │
│                     │                                                       │
└─────────────────────┼───────────────────────────────────────────────────────┘
                      │
                      │ TLS (handled by Anthropic/vendor)
                      ▼
               ┌─────────────┐
               │   AI VM     │
               │ (Claude,    │
               │  GPT, etc.) │
               └─────────────┘
```

---

## Supported Commands

Only remote Git operations that require credential injection are proxied:

| Command | Description |
|---------|-------------|
| `clone` | Clone a repository |
| `fetch` | Download objects and refs from a remote |
| `pull` | Fetch and integrate with a remote |
| `push` | Update remote refs |
| `ls-remote` | List references in a remote repository |

**Local commands** (`status`, `log`, `diff`, `add`, `commit`, `branch`, etc.) are intentionally **not supported**.
AI assistants can run these directly — they don't need credential injection.

---

## Features

| Feature | Status |
|---------|--------|
| Credential isolation | Planned |
| GitHub/GitLab support | Planned |
| Remote-only command proxy | ✅ Implemented |
| SSH key support | Planned |
| Audit logging | Planned |
| Protected branch guardrails | Planned |
| Git LFS support | Future |

See [TODO.md](TODO.md) for the full roadmap.

---

## Installation

> **Note:** git-proxy-mcp is in early development. Installation instructions will be added when the first release is available.

### From Source (Development)

```bash
# Clone the repository
git clone https://github.com/MatejGomboc/git-proxy-mcp.git
cd git-proxy-mcp

# Build
cargo build --release

# Run
./target/release/git-proxy-mcp
```

### Requirements

- Rust 1.70+ (for building from source)
- Git 2.x
- Supported platforms: Windows, macOS, Linux

---

## Configuration

Create a `config.json` file with your repository credentials:

```json
{
    "credentials": [
        {
            "url_pattern": "github.com/*",
            "auth": {
                "type": "pat",
                "token": "ghp_xxxxxxxxxxxx"
            }
        },
        {
            "url_pattern": "gitlab.company.com/*",
            "auth": {
                "type": "ssh",
                "key_path": "~/.ssh/id_ed25519"
            }
        }
    ]
}
```

See [config/example-config.json](config/example-config.json) for a complete example.

---

## Usage with MCP Clients

### Claude Desktop

Add to your Claude Desktop MCP configuration:

```json
{
    "mcpServers": {
        "git-proxy": {
            "command": "git-proxy-mcp",
            "args": ["--config", "/path/to/config.json"]
        }
    }
}
```

### Other MCP Clients

git-proxy-mcp uses stdio transport, compatible with any MCP client that supports local server processes.

---

## Security Model

**Core guarantee:** Credentials are loaded from config, used internally for Git operations, and **never** serialised to MCP responses.

### What flows to the AI

- Repository file contents
- Commit history and metadata
- Branch and tag information
- Operation status (success/failure)

### What stays local

- Personal Access Tokens
- SSH private keys
- Any authentication secrets
- Credential configuration

### Audit it yourself

This project is open source under GPL-3.0. Review the code, verify the credential handling, and build from source if you prefer.

---

## Contributing

Contributions are welcome! Please read [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines.

- Follow the style guide in [STYLE.md](STYLE.md)
- Security issues: see [SECURITY.md](SECURITY.md)

---

## Licence

Copyright (C) 2025 Matej Gomboc <https://github.com/MatejGomboc/git-proxy-mcp>.

This program is free software: you can redistribute it and/or modify
it under the terms of the GNU General Public License as published by
the Free Software Foundation, either version 3 of the License, or
(at your option) any later version.

This program is distributed in the hope that it will be useful,
but WITHOUT ANY WARRANTY; without even the implied warranty of
MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
GNU General Public License for more details.

See the attached [LICENCE](LICENCE) file for more info.

---

## Links

- [MCP Specification](https://modelcontextprotocol.io/)
- [Report an Issue](https://github.com/MatejGomboc/git-proxy-mcp/issues)
