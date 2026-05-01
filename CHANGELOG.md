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
- **Codecov upload — `fail_ci_if_error: true`** — both `ci_main.yml`
  and `ci_pr.yml` now fail the job loudly when the Codecov upload
  fails, instead of silently going green. The `CODECOV_TOKEN` repo
  secret has been added (previously missing — every upload was being
  rejected with HTTP 400 "Token required - not valid tokenless upload"
  but `fail_ci_if_error: false` masked this, leaving the badge stuck
  at "unknown"). OIDC was attempted first but the Codecov ingest
  endpoint reproducibly returned HTTP 500 for this repo + OIDC
  combination — token-based auth is the working path until Codecov's
  OIDC handling matures.
- **Coverage merging — tests + report generation share one shell** —
  the `coverage` job in `ci_main.yml` previously split its work
  across multiple shell steps: unit tests in one, integration tests
  in another, and `cargo llvm-cov report` in a third. Each step
  re-entered a fresh shell where the env vars `cargo llvm-cov
  show-env --sh` exports (most importantly `LLVM_PROFILE_FILE` and
  `CARGO_LLVM_COV_TARGET_DIR`) were no longer set — so binaries
  wrote profraw to one location and `report` looked in another.
  Net effect: PR #127's "merged unit + integration coverage"
  feature silently produced unit-test-only numbers (~77% lines)
  despite the integration tests running fine. Now everything runs
  in a single shell: `source show-env`, clean stale profraw, run
  unit tests, build instrumented release binary, run integration
  tests, list the resulting profraw for diagnostic visibility,
  generate the lcov + summary reports — all sharing the same env.
  The next `Upload coverage to Codecov` step (a separate shell)
  only needs `lcov.info` from disk, which persists across steps.
- **LFS error diagnostics** — three small observability improvements to make
  LFS resolution failures actionable from CI logs alone:
  1. `LfsClient::new` now logs a `WARN` if no credentials were provided
     (private repos will return 401/403 — easier to spot than the eventual
     batch-API error).
  2. The non-retryable batch-POST error path now reads the response body
     (which GitHub/GitLab use for structured error messages such as
     "Bad credentials" or "Repository not found") and includes it both in
     a `WARN` log line and the returned error string. The Authorization
     header is in the request, never the response — so this never echoes
     the PAT.
  3. `tests/integration/test_mcp_tools.py` now dumps the **full** server
     log on test-suite failure instead of `tail -50`, so the LFS batch
     status code and body (logged at the *start* of each clone, before
     any progress noise) are visible in the GitHub Actions step output.

### Fixed

- Dependabot CI failures caused by orphaned `dtolnay/rust-toolchain` SHAs. The
  previous `# stable` pin pointed to a commit that fell off the remote when
  the rolling `stable` branch advanced, breaking Dependabot's
  `git branch --remotes --contains <sha>` lookup.
- **MCP tool names documented with the correct underscore form.** All
  documentation, Rust doc comments, and CHANGELOG references previously
  used the slash form (`repo/clone`, `repo/push`, etc.) as if these were
  JSON-RPC method names. They are not — they are tool names registered
  via `tools/list` and dispatched via `tools/call`. The actual code has
  always registered them with underscores (`repo_clone`, `repo_push`,
  `repo_clone_status`, etc. — see the dispatch table in
  `src/mcp/server.rs`). This was a purely documentation-side bug —
  no runtime behaviour change. Affected files: `README.md`,
  `docs/{AI_WORKFLOW,ARCHITECTURE,SECURITY,VISION,errors}.md`, the
  `[1.1.0]` section of `CHANGELOG.md`, and the doc comments in
  `src/{lib,mcp/server,security/audit,streaming/chunked}.rs`. ~75
  occurrences corrected. Tool names in actual code (string literals
  in the dispatch table) were already correct and unchanged.
- **Coverage job binary path** — the `Build instrumented release binary and
  run integration tests` step in `ci_main.yml` referenced `$CARGO_TARGET_DIR`,
  which `cargo llvm-cov show-env` has not exported since 0.1.14 (Jan 2022).
  The expansion collapsed to `/release/git-proxy-mcp`, causing every push to
  main since PR #127 merged to fail with `FileNotFoundError`. Now uses
  `$CARGO_LLVM_COV_TARGET_DIR` (the workspace target directory exposed by
  current cargo-llvm-cov) and switches `--export-prefix` to its non-deprecated
  `--sh` alias.
- **LFS endpoint URL incorrectly stripped `.git` suffix** — `derive_lfs_url`
  in `src/git2_ops/lfs.rs` was calling `trim_end_matches(".git")` before
  appending `/info/lfs`. GitHub's LFS service is mounted at
  `<repo>.git/info/lfs/objects/batch`; without `.git`, the request reaches
  GitHub's web frontend instead and gets a 422 + HTML response, causing
  `lfs_failed += 1` for every LFS pointer. Removed the strip — the URL is
  now passed through verbatim, matching the canonical `git-lfs` client
  behaviour. Existing `*_no_git_suffix` tests still pass (those URLs never
  had a suffix to begin with); the four tests that asserted the stripped
  form have been updated to expect the correct preserved form.

### Security

- **`py/command-line-injection` (CodeQL alert #2, CWE-78/CWE-88)** —
  closed at the source by removing the env-var input entirely.
  `tests/integration/test_mcp_tools.py` previously took the binary
  path from `GIT_PROXY_MCP_BINARY` and ran it through a three-layer
  `_sanitise_binary_path` validator (regex + canonicalise + allowlist
  to `<repo>/target/release`). CodeQL re-flagged the call site after
  unrelated edits shifted line numbers; rather than maintain a sanitiser
  for an input that never had a real use case, drop the env-var input
  altogether. The binary path is now hard-coded as
  `os.path.realpath("./target/release/git-proxy-mcp")`. The coverage
  workflow's instrumented release build lands at the same path
  (cargo-llvm-cov stopped overriding `CARGO_TARGET_DIR` in 0.1.14),
  so the override the workflow used to pass via `GIT_PROXY_MCP_BINARY`
  was vestigial — also removed from `ci_main.yml`. No taint flow from
  `os.environ` to `subprocess.Popen` remains.

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
  `submodule_exclude` tool arguments for `repo_clone` and `repo_clone_start`.
- **Parallel submodule fetching** — submodules at each depth level are now
  fetched in parallel using `std::thread::scope`, controlled by the existing
  `max_concurrent` setting (default 4). When `max_concurrent` is 1, behaviour
  degrades gracefully to sequential fetching.
- **Chunk-level resume** — new `repo_clone_status` tool to check progress and identify
  missing chunks in a Tier 2 streaming session. `repo_clone_chunk` responses now include
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
