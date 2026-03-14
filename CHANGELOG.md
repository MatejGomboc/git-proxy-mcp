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
- **Configurable session management** — new `sessions` section in `config.json` for
  setting session timeout, max concurrent streaming sessions, and max repo sessions.
  Previously hardcoded to 1 hour / 10 / 100 respectively.
- **LFS improvements** — new `lfs` section in `config.json` with retry logic
  (exponential backoff for transient failures), per-object and total size limits,
  and byte-level progress tracking during LFS downloads.
- **Submodule improvements** — recursive submodule fetching with per-request
  depth control (unlimited by default, mirroring git), include/exclude glob
  pattern filtering, cycle detection, and early termination after configurable
  failure count. New `submodule_depth`, `submodule_include`, and
  `submodule_exclude` tool arguments for `repo/clone` and `repo/clone_start`.
- **Parallel submodule fetching** — submodules at each depth level are now
  fetched in parallel using `std::thread::scope`, controlled by the existing
  `max_concurrent` setting (default 4). When `max_concurrent` is 1, behaviour
  degrades gracefully to sequential fetching.
- **Chunk-level resume** — new `repo_clone_status` tool to check progress and identify
  missing chunks in a Tier 2 streaming session. `repo/clone_chunk` responses now include
  `next_missing_chunk` so the AI can resume interrupted transfers without re-downloading
  chunks it already has.

### Changed

- **CI/CD improvements** — added concurrency groups to cancel superseded runs, added
  `timeout-minutes` to all jobs, deduplicated cache cleanup logic, narrowed PR trigger
  to `main` branch only, improved cancellation handling with `!cancelled()`.
- Pre-release tags (e.g., `v1.0.0-rc1`) no longer get marked as the "latest" GitHub
  release.
- Submodule recursion depth is now a per-request tool argument (`submodule_depth`)
  rather than a server config setting, mirroring how `git clone --recurse-submodules`
  works. Default is unlimited (git default).
- Branch default description updated to "remote's default branch" instead of
  hardcoded "main", matching git's actual behaviour.

### Fixed

- Pinned all GitHub Actions to full-length commit SHAs, as required by the repository's
  action permissions settings.
- Pinned `dtolnay/rust-toolchain` to commit SHA with explicit `toolchain: stable` input.

### Security

- Upgraded `git2` from `0.19` to `0.20.4` to fix potential undefined behaviour when
  dereferencing `Buf` struct (CWE-476).

## [1.0.0] - 2025-01-10

Initial release.
