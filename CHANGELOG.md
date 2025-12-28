# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Initial project structure and build configuration
- Development container configuration for VS Code/Codespaces
- CI/CD workflow for multi-platform builds (Ubuntu, macOS, Windows)
- CLI skeleton with argument parsing (`clap`)
- Example configuration file demonstrating all auth types
- Comprehensive development documentation (`TODO.md`, `JOURNAL.md`)
- Feature tracking system (`features.json`)
- Security policy (`SECURITY.md`)
- Contributing guidelines (`CONTRIBUTING.md`)

### Security

- Configured strict clippy lints including `unsafe_code = "forbid"`
- Added `secrecy` crate for secure credential handling
- Established credential isolation architecture (credentials never in MCP responses)

---

## Version History

This project is in early development. No releases yet.

### Versioning Policy

Once v1.0.0 is released, we will follow semantic versioning:

- **MAJOR** version for incompatible API/config changes
- **MINOR** version for backwards-compatible functionality additions
- **PATCH** version for backwards-compatible bug fixes

### Pre-release Versions

- `0.x.y` versions may have breaking changes between minor versions
- Always check the changelog before upgrading pre-release versions

---

[Unreleased]: https://github.com/MatejGomboc/git-proxy-mcp/compare/main...HEAD
