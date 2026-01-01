# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [Unreleased]

### Added

- MCP server infrastructure with JSON-RPC 2.0 protocol support
- Git command parsing and validation for remote operations (clone, fetch, pull, push, ls-remote)
- Git command executor with credential injection
- Configuration system with credential management
- Security guards for protected branches and force push prevention
- Rate limiting implementation
- Audit logging framework
- Output sanitisation to remove credentials from git output
- Cross-platform CI/CD pipeline (Windows, macOS, Linux)
- GitHub Actions release workflow with automated binary builds

### In Progress

- MCP server integration (wiring executor to tool handler)
- End-to-end git command execution via MCP

### Security

- Credential isolation — PATs and SSH keys never leave the user's machine
- Secure string handling with automatic zeroisation
- Command sanitisation to prevent injection attacks
