# TODO — Development Battle Plan

## Overview

**Goal:** Build a secure, AI-agnostic Git proxy MCP server in Rust that keeps credentials on the user's machine
while allowing AI assistants to work with repos in their own environments.

**Guiding Principles:**

- Security over speed. Take the time to do it right.
- Work on ONE feature at a time.
- Follow the style guide in `STYLE.md` and contributor guidelines in `CONTRIBUTING.md`.

**For AI Assistants:** See `.claude/CLAUDE.md` for project context.

---

## Security Architecture

### Credential Isolation — CRITICAL

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

**Key Security Properties:**

1. Credentials are loaded from config, used internally, and NEVER serialised to MCP responses
2. stdio transport = local process communication, no network between MCP server and client
3. Only git pack data (file contents, commits, branches) flows through MCP
4. Anthropic/vendor handles encryption between their client and AI VM

---

## Design Decisions (Locked In)

| Decision | Choice | Rationale |
|----------|--------|----------|
| Config hot-reload | No | Security: config changes require restart to prevent runtime injection |
| Concurrent operations | Yes | Allow multiple repos to be accessed simultaneously |
| Timeline priority | Security first | Take time to do it right, no rushing |
| Transport | stdio only (v1) | Simplest, most secure for local MCP clients |
| SSH keys | User manages | User sets up keys on PC, we reference path or use ssh-agent |
| Large repos | Stream output | Forward Git's stdout/stderr in real-time to MCP client |
| Git LFS | Defer to v1.1 | v1.0: detect & warn; v1.1+: implement support |
| Feature tracking | `TODO.md` | Single source of truth for roadmap and progress |
| Proxy approach | Pass-through | Proxy Git CLI commands, inject credentials, return output |
| Scope | Git CLI only | Web UI features (PRs, issues, etc.) are out of scope |
| Command scope | Remote-only | Only clone/fetch/pull/push/ls-remote; local commands run directly |

---

## Phase 5: SSH & Remote Improvements <- CURRENT

- [ ] Named remote resolution (e.g., "origin" -> actual URL from .git/config)

---

## Phase 6: Code Quality & Cleanup

- [ ] Remove unused dependencies from Cargo.toml (git2, anyhow, url)
- [ ] Optimise tokio features (currently using "full", only need subset)
- [ ] Audit codebase for British spelling consistency (see CONTRIBUTING.md)
- [ ] Convert ASCII diagrams to Mermaid (TODO.md, README.md)

---

## Phase 7: Testing & Documentation

- [ ] Integration tests for credential injection (currently only unit tests)
- [ ] Integration tests for full MCP -> git pipeline
- [ ] Tests for large git output handling
- [ ] Documentation for error messages and error codes

---

## Phase 8: Robustness & Production Readiness

- [ ] Request timeout configuration (prevent hung git processes)
- [ ] Graceful shutdown handling (SIGTERM/SIGINT)
- [ ] Output size limits (prevent protocol buffer overflow)
- [ ] Configurable rate limiting (currently hardcoded: 20 burst, 5/sec)

---

## Phase 9: Cross-Platform Release

- [x] GitHub Actions release workflow
- [x] Build targets (Windows x64, macOS x64/ARM64, Linux x64)
- [ ] Binary signing (if applicable)
- [x] Semantic versioning and CHANGELOG maintenance
- [x] User documentation (installation guide, configuration reference)
- [x] Example MCP client configurations (Claude Desktop, etc.)

---

## Future Considerations (v1.1+)

- Git LFS support (currently detect & warn only)
- Structured logging with JSON output option
- Metrics/telemetry endpoint
- Per-operation audit trail with session tracking
- Health check endpoint for monitoring

---

## References

- **MCP Specification:** <https://modelcontextprotocol.io/>
- **Open Source Guides:** <https://opensource.guide/>
- **Claude Code Docs:** <https://docs.anthropic.com/en/docs/claude-code>
- **Swatinem/rust-cache:** <https://github.com/Swatinem/rust-cache>
- **EditorConfig:** <https://editorconfig.org/>

---

*Last updated: 2026-01-01*
