# TODO — Development Battle Plan

## Overview

**Goal:** Build a secure, AI-agnostic Git proxy MCP server in Rust that spawns git commands on behalf of AI assistants, using the user's existing git credential configuration.

**Guiding Principles:**

- Security over speed. Take the time to do it right.
- Work on ONE feature at a time.
- Follow the style guide in `STYLE.md` and contributor guidelines in `CONTRIBUTING.md`.

**For AI Assistants:** See `.claude/CLAUDE.md` for project context.

---

## Security Architecture

### Credential-Free Proxy Design

```
User's PC (existing git setup)
├── ~/.gitconfig (credential helpers)
├── ~/.ssh/config (SSH host configs)
├── ssh-agent (keys loaded)
└── OS credential store

          ↓ git uses these automatically

git-proxy-mcp:
├── Validate command (security guards)
├── Spawn: git clone/fetch/pull/push/ls-remote
├── GIT_TERMINAL_PROMPT=0 (no interactive prompts)
├── Sanitise output (remove any leaked credentials)
└── Return result to AI via MCP

          ↓ stdio (local process)

Claude Desktop / MCP Client

          ↓ TLS (handled by vendor)

AI VM (Claude, GPT, etc.)
```

**Key Security Properties:**

1. MCP server stores NO credentials — uses git's native credential system
2. User configures git once, same as they would for manual use
3. stdio transport = local process communication, no network exposure
4. Only git output flows through MCP, sanitised for safety

---

## Design Decisions (Locked In)

| Decision | Choice | Rationale |
|----------|--------|----------|
| Credential storage | None | Use git's native credential helpers; no duplication |
| Config hot-reload | No | Security: config changes require restart |
| Concurrent operations | Yes | Allow multiple repos to be accessed simultaneously |
| Transport | stdio only (v1) | Simplest, most secure for local MCP clients |
| SSH keys | User manages via ssh-agent | Standard tooling, no MCP involvement |
| Git LFS | Defer to v1.1 | v1.0: detect & warn; v1.1+: implement support |
| Proxy approach | Pass-through | Spawn git subprocess, return output |
| Scope | Git CLI only | Web UI features (PRs, issues, etc.) are out of scope |
| Command scope | Remote-only | Only clone/fetch/pull/push/ls-remote |

---

## Completed: Phase 5 — Remove Credential Storage

Major architectural simplification completed. The MCP server no longer stores credentials.
Instead, it relies on the user's existing Git configuration (credential helpers, SSH agent).

- [x] Remove `src/auth/` module entirely
- [x] Remove `remotes` section from config
- [x] Simplify `GitExecutor` to just spawn git
- [x] Remove `secrecy` crate dependency
- [x] Update config to security/logging settings only
- [x] Update README with credential-free approach
- [x] Update example config
- [x] Fix all tests

---

## Phase 6: Code Quality & Cleanup <- CURRENT

- [x] Optimise tokio features (currently using "full", only need subset)
- [ ] Audit codebase for British spelling consistency (see CONTRIBUTING.md)
- [ ] Convert ASCII diagrams to Mermaid (TODO.md, README.md)

---

## Phase 7: Testing & Documentation

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
