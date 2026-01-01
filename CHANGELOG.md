# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [Unreleased]

### Added

- MCP server with JSON-RPC 2.0 protocol support
- Git command proxy for remote operations (clone, fetch, pull, push, ls-remote)
- Configuration system with credential management
- Security guards for protected branches and force push prevention
- Rate limiting for Git operations
- Audit logging framework
- Cross-platform CI/CD pipeline (Windows, macOS, Linux)
- GitHub Actions release workflow with automated binary builds

### Security

- Credential isolation — PATs and SSH keys never leave the user's machine
- Secure string handling with automatic zeroisation
- Command sanitisation to prevent injection attacks
