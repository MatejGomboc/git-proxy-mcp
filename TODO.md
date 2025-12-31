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
                      │ 🔒 TLS (handled by Anthropic/vendor)
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
| Config hot-reload | ❌ No | Security: config changes require restart to prevent runtime injection |
| Concurrent operations | ✅ Yes | Allow multiple repos to be accessed simultaneously |
| Timeline priority | Security first | Take time to do it right, no rushing |
| Transport | stdio only (v1) | Simplest, most secure for local MCP clients |
| SSH keys | User manages | User sets up keys on PC, we reference path or use ssh-agent |
| Large repos | Stream output | Forward Git's stdout/stderr in real-time to MCP client |
| Git LFS | Defer to v1.1 | v1.0: detect & warn; v1.1+: implement support |
| Feature tracking | `TODO.md` | Single source of truth for roadmap and progress |
| Proxy approach | Pass-through | Proxy Git CLI commands, inject credentials, return output |
| Scope | Git CLI only | Web UI features (PRs, issues, etc.) are out of scope |

---

## Phase 3: Git Command Proxy ← CURRENT

- [ ] Parse and validate incoming Git commands
- [ ] Match remote URL to configured credentials
- [ ] Inject credentials via Git credential helper or environment
- [ ] Execute Git command as subprocess
- [ ] Capture and return stdout, stderr, exit code
- [ ] Sanitize output to prevent credential leakage
- [ ] LFS detection and warning

---

## Phase 4: Security & Safety

- [ ] Audit logging to file
- [ ] Protected branch enforcement
- [ ] Force push blocking
- [ ] Repository allowlist/blocklist enforcement
- [ ] Operation rate limiting (prevent runaway AI operations)

---

## Phase 5: Integration Testing

- [ ] Integration tests with mock git server
- [ ] End-to-end tests for Git command proxy
- [ ] Security tests (credential leak detection in output)
- [ ] Manual testing with MCP clients (Claude Desktop)

---

## Phase 6: Cross-Platform Release

- [ ] GitHub Actions release workflow
- [ ] Build targets (Windows, macOS, Linux)
- [ ] Binary signing (if applicable)
- [ ] Semantic versioning and CHANGELOG maintenance
- [ ] User documentation (installation guide, configuration reference)
- [ ] Example MCP client configurations (Claude Desktop, etc.)

---

## References

- **MCP Specification:** <https://modelcontextprotocol.io/>
- **git2-rs Documentation:** <https://docs.rs/git2>
- **Open Source Guides:** <https://opensource.guide/>
- **Claude Code Docs:** <https://docs.anthropic.com/en/docs/claude-code>
- **Swatinem/rust-cache:** <https://github.com/Swatinem/rust-cache>
- **EditorConfig:** <https://editorconfig.org/>

---

*Last updated: 2025-12-31*
