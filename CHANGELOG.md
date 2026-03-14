# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **Proxy support** — new `proxy` section in `config.json` for routing all network
  operations (git fetch/push/connect, LFS downloads) through HTTP, HTTPS, or SOCKS5
  proxy servers. Includes `no_proxy` for bypassing the proxy on specific hosts.
  Useful for corporate environments behind firewalls.

### Changed

- **CI/CD improvements** — added concurrency groups to cancel superseded runs, added
  `timeout-minutes` to all jobs, deduplicated cache cleanup logic, narrowed PR trigger
  to `main` branch only, improved cancellation handling with `!cancelled()`.
- Pre-release tags (e.g., `v1.0.0-rc1`) no longer get marked as the "latest" GitHub
  release.

### Fixed

- Pinned all GitHub Actions to full-length commit SHAs, as required by the repository's
  action permissions settings.
- Pinned `dtolnay/rust-toolchain` to commit SHA with explicit `toolchain: stable` input.

### Security

- Upgraded `git2` from `0.19` to `0.20.4` to fix potential undefined behaviour when
  dereferencing `Buf` struct (CWE-476).

## [1.0.0] - 2025-01-10

Initial release.
