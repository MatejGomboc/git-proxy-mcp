# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **Integration coverage merging** — the `coverage` job in `ci_main.yml` now
  builds an instrumented release binary (via `cargo llvm-cov show-env`) and
  runs the Python integration tests against it. The resulting coverage data
  merges with the unit-test data into a single Codecov upload. The standalone
  `integration` job has been removed; its responsibilities are now part of
  `coverage` (avoids duplicate fixture rebuild and the resulting force-push
  race condition).
- **Mock LFS server tests** — 7 new Rust unit tests in `git2_ops::lfs::tests`
  using `mockito` to verify retry-on-5xx, no-retry-on-4xx, exhaust-after-N,
  oversized-object pre-check, batch-API error mapping, and Basic auth header
  emission. Previously these paths only ran against a real LFS endpoint.
- **Property-based tests** — new `tests/property_tests.rs` using `proptest`
  for URL sanitisation (no panic, bounded growth, credential-stripping
  invariants), URL validation (schema rejection consistency), and LFS
  pointer detection/parsing (no panic on adversarial bytes, valid-input
  round-tripping). 11 properties run with 256 cases each by default.
- **Expanded integration test fixture** — fixture now includes a renamed-file
  commit (commit 4: `docs/DESIGN.md` → `docs/ARCHITECTURE.md`) and an
  LFS-tracked binary file (commit 5: `docs/large.bin`). Adds 4 integration
  tests covering rename detection in diff/pull and LFS pointer resolution.
- CI workflows now install `git-lfs` before rebuilding the test fixture.
- **Code coverage reporting** — `cargo-llvm-cov` runs on every PR and push to main,
  uploading lcov reports to Codecov. Patch coverage target is 80% for changed lines;
  project coverage cannot drop more than 1% on main. Coverage badge added to README.
- **501 unit tests** (up from 257 baseline) covering URL validation, error paths,
  argument parsing, server request routing, tar archive creation with local bare
  repos, LFS pointer parsing, commit resolution from branch/tag/SHA refs, commit
  counting between two refs, file archive creation from trees, `.gitmodules`
  parsing with realistic content, and progress notification serialisation.
- Coverage rose from ~58% (baseline) to ~76% lines and ~84% functions, measured
  excluding `main.rs` (CLI entry point) and `transport.rs` (stdio I/O wrapper) —
  both excluded via `codecov.yml` since they cannot be unit-tested without
  invasive refactoring; their behaviour is verified end-to-end by the Python
  integration tests against the real fixture repo.

### Changed

- **Reproducible Rust toolchain pin** — `dtolnay/rust-toolchain` is now pinned
  to the immutable per-version branch SHA (`v1.95.0`) instead of the rolling
  `stable` branch, and `rust-toolchain.toml` is pinned to the matching version.
  CI and local development builds now use the same compiler. A new pre-flight
  check (`.github/scripts/check-toolchain-pin.sh`) fails the quick-checks job
  if the action pin and the `rust-toolchain.toml` channel disagree, so future
  toolchain bumps must update both together.

### Fixed

- Dependabot CI failures caused by orphaned `dtolnay/rust-toolchain` SHAs. The
  previous `# stable` pin pointed to a commit that fell off the remote when
  the rolling `stable` branch advanced, breaking Dependabot's
  `git branch --remotes --contains <sha>` lookup.

## [1.1.0] - 2026-03-14

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
- **Integration tests** — Python-based end-to-end tests (`tests/integration/`) that
  rebuild a private test fixture repo from scratch and exercise all 10 MCP tools
  (initialise, refs, clone, diff, pull, push, Tier 2 streaming lifecycle, helper
  script) against a real GitHub remote, including error handling, edge cases,
  credential leak checks, protected branch enforcement, and sparse clone
  verification. Runs as part of `ci_main.yml` on every push to main, plus nightly
  and on manual dispatch via `ci_integration.yml`.

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
- Accept git >= 2.53 bundle header format (`# v2 git bundle` / `# v3 git bundle`).
- Use `git` CLI for bundle unbundling instead of libgit2 (which does not support
  fetching from bundle files reliably across platforms).
- Added bundle size limit (1 GiB) to prevent memory exhaustion via oversized bundles.

### Security

- Upgraded `git2` from `0.19` to `0.20.4` to fix potential undefined behaviour when
  dereferencing `Buf` struct (CWE-476).

## [1.0.0] - 2025-01-10

Initial release.
