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
- **510 unit tests** (up from 257 baseline) covering URL validation, error paths,
  argument parsing, server request routing, tar archive creation with local bare
  repos, LFS pointer parsing, commit resolution from branch/tag/SHA refs, commit
  counting between two refs, file archive creation from trees, `.gitmodules`
  parsing with realistic content, and progress notification serialisation.
- Coverage rose from ~58% (baseline) to ~77% lines and ~84% functions, measured
  excluding `main.rs` (CLI entry point) and `transport.rs` (stdio I/O wrapper) —
  both excluded via `codecov.yml` since they cannot be unit-tested without
  invasive refactoring; their behaviour is verified end-to-end by the Python
  integration tests against the real fixture repo.

### Changed

- **Config-drift audit** — comprehensive cross-reference of every config
  option across `src/config/settings.rs` (canonical), `config/example-config.json`
  (full reference), and the README configuration table. All 26
  user-configurable options checked: defaults match exactly across all
  three sources, no ghost entries, no type/unit mismatches. Two cosmetic
  alignments applied:
  1. **`Config` struct field order** rearranged to match the user-facing
     order in `example-config.json` and the README (`git_identity`
     first, then `security`, `logging`, ..., `submodules`). Field names
     drive serde deserialisation, so order is purely cosmetic — but a
     reader with the README open now sees the source laid out the same
     way.
  2. **Six terse field rustdocs brought up to README detail.** The most
     material gap was `SecurityConfig.protected_branches`: its rustdoc
     said only "List of protected branch names." with no mention of the
     empty-list-fallback behaviour (`McpServer::new` substitutes
     `BranchGuard::with_defaults()` → `main`/`master`/`develop` when the
     config list is empty). PR #150 documented this in the README,
     `docs/errors.md`, and `BranchGuard::with_defaults` rustdoc but
     didn't update the field that triggers the behaviour, so
     `cargo doc --document-private-items` (which CI publishes) showed
     less detail on this field than every other user-facing source.
     Same treatment applied to the five sibling fields (`allow_force_push`,
     `repo_allowlist`, `repo_blocklist`, `level`, `audit_log_path`),
     bringing them in line with the already-detailed rustdocs on
     `TimeoutConfig`, `LimitsConfig`, `RateLimitConfig`, `SessionConfig`,
     `LfsConfig`, `SubmoduleConfig`, and `ProxyConfig`.
- **CI/CD audit sweep** — comprehensive pass across every workflow and
  helper script in `.github/`, fixing one real cache bug and a handful
  of hardening/hygiene issues:
  1. **Permissions tightened to least-privilege.** `ci_main.yml` had
     `actions: write` at workflow level inherited by every job — only
     `ci-success` actually uses it (cache-cleanup REST API), so the
     scope was moved to that job alone. `release.yml` had `contents:
     write` + `id-token: write` + `attestations: write` at workflow
     level inherited by the read-only `validate` and `build` jobs;
     now only the `release` job (which creates the GitHub release
     and submits SLSA build provenance) carries elevated scope.
  2. **Concurrency groups added** to `release.yml` (`group:
     release-${{ github.ref }}`, `cancel-in-progress: false`) and
     `cleanup_caches.yml` (`group: cleanup-caches`), so re-running
     a tag job or kicking off two cache cleanups at once doesn't
     race on the GitHub release page or the cache-delete API.
  3. **PAT no longer inlined into shell.** The integration-coverage
     credential setup in `ci_main.yml` now passes
     `${{ secrets.TEST_REPO_PAT }}` via an `env:` block and reads
     it as `${TEST_REPO_PAT}` in the script body, keeping the secret
     value out of the rendered run-script source (GitHub's documented
     best practice).
  4. **`cleanup_caches.yml` `dry_run` default flipped to `true`** so
     an accidental "Run workflow" click doesn't actually delete
     caches; the input description now spells out the destructive
     case ("uncheck to actually delete").
  5. **`release.yml` `Cargo.toml` version extraction hardened** —
     `grep '^version = ' | head -1 | sed` was replaced with awk that
     explicitly tracks the `[package]` section and exits on first
     match, so a stray `version = "..."` under another table
     can't be picked up by accident.
  6. **`gh release create` flag building** switched from shell
     word-splitting on `$LATEST_FLAG` / `$PRERELEASE_FLAG` to a
     `RELEASE_FLAGS=()` bash array — shellcheck-friendly and robust
     if values ever contain spaces.
  7. **`ci-success` bash style unified** between `ci_pr.yml` and
     `ci_main.yml` (bare `||`/`&&` line continuations, no `\`),
     and the unused `id: check-results` on `ci_main.yml`'s
     ci-success step was removed.
  8. **`cleanup-caches.js` size labels** changed from KB/MB/GB to
     KiB/MiB/GiB to match the rest of the project's binary-unit
     reporting after the PR #150 sweep, and `Math.pow(k, i)` was
     modernised to `k ** i`.
  9. **`check-toolchain-pin.sh` now resolves paths relative to the
     script's own location** rather than relying on the caller's
     CWD being the repo root. Also exports `LC_ALL=C` so `sort -u`
     orders bytes consistently across runners. Verified locally
     from both the repo root and `/tmp`.
  10. **`ci_integration.yml` deleted** — it was a `workflow_dispatch`-
      only duplicate of the integration testing already running in
      `ci_main.yml`'s `coverage` job. The inline comment in
      `ci_main.yml` already documented that the dedicated integration
      job had been merged into coverage to avoid the
      `rebuild_fixture.py` force-push race condition; the standalone
      file was the last remnant.
- **`reqwest` requirement bumped 0.12 → 0.13** (cargo-dependencies group).
  reqwest 0.13 split the TLS feature: the old `rustls-tls` umbrella (which
  0.12 expanded to `__rustls` + `webpki-roots`) no longer exists; callers
  must opt into `rustls` plus an explicit cert source. Updated `Cargo.toml`
  to `["rustls", "webpki-roots", "json", "blocking", "socks"]` — same
  trust-anchor set as before (Mozilla's root store, embedded at build
  time), so LFS HTTPS requests behave identically. None of the reqwest
  API surface used by `LfsClient` (`Client`, `Proxy`, `NoProxy`,
  `HeaderMap`, `StatusCode`, `Response`, `Error`) changed shape.
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

- **CI Rust cache key keyed on a gitignored file.** Every Rust cache
  step in `ci_pr.yml`, `ci_main.yml`, and `ci_integration.yml` keyed
  on `hashFiles('**/Cargo.lock')`, but the project's `.gitignore`
  excludes `Cargo.lock`. On a freshly-checked-out runner the lock
  file doesn't exist yet at the time the cache action runs (the
  action runs *before* `cargo build` regenerates it), so `hashFiles`
  returned the hash-of-no-files sentinel — a fixed value that never
  changed when dependencies did. Net effect: the cache key collapsed
  to `rust-${OS}-${rustc-hash}-` and never invalidated when
  `Cargo.toml` was edited (e.g. the recent reqwest 0.12 → 0.13
  bump in PR #149). Correctness was preserved because cargo's own
  fingerprint check detects mismatched deps and rebuilds, but the
  cache became a write-heavy no-op rather than a speedup. Switched
  all five cache-key lines to `hashFiles('Cargo.toml')` — checked
  in, exists at action time, and changes exactly when dependencies
  do.
- **`fetch_bare` hardcoded the fallback branch to `main` despite docs
  promising "the remote's default branch".** When `repo_clone` /
  `repo_clone_start` was called without a `branch` argument,
  `git2_ops::clone::fetch_bare` resolved the missing branch to a literal
  `"main"` (`options.branch.as_deref().unwrap_or("main")`), so any repo
  with a non-`main` default (e.g. `master`, `develop`, or a renamed
  default) would return `RefNotFound("main")` instead of fetching. The
  `[1.1.0]` CHANGELOG entry that described this as fixed was true at
  the docs/schema level only — the `tools/list` description and the
  `RepoCloneArgs.branch` rustdoc had been updated to "remote's default
  branch", but the code had never been changed to match.
  Now `fetch_bare` opens a single `connect_auth` to the remote, asks
  `Remote::default_branch()` when the caller passed no branch, strips
  the leading `refs/heads/`, and proceeds with the fetch over the same
  connection. Caller-supplied branches still take precedence and skip
  the probe. There is no longer a hard-coded `"main"` fallback —
  malformed remotes (no `HEAD` symref) now surface a proper
  `FetchFailed("could not determine remote's default branch: …")`
  error instead of silently pretending the default is `main`.
  No change to integration tests (they all pass `branch: "main"`
  explicitly), and the existing in-repo unit test is unaffected since
  it only checks `FetchOptions2` defaults.
- **`helper_script` Python helper looked for the wrong `repo_pull` archive
  field name.** The embedded `git_proxy_helper.py` script's `extract` and
  `info` commands expected the `repo_pull` response to carry the archive
  under a `changed_files_archive` key, but the actual response field has
  always been named `files_archive` (declared in `RepoPullResult` since
  v1.1.0 — see `src/mcp/tools/repo_pull.rs`). Net effect: any AI assistant
  that used the helper script to extract a `repo_pull` result hit
  `ValueError: No archive found in result` and fell back to manual
  base64+tar handling. Fixed by renaming both lookups to `files_archive`
  and updating the resulting `info` key to `has_files_archive`. The
  `archive` field used for `repo_clone` results was always correct and is
  unchanged. Added a regression test
  (`helper_script_uses_correct_repo_pull_archive_field`) that asserts
  the script references `files_archive` and never the obsolete
  `changed_files_archive`.
- **`src/mcp/tools/mod.rs` rustdoc listed only 8 of the 10 MCP tools.**
  The `Tier 2 (Chunked Streaming)` section in the module-level doc-comment
  named `repo_clone_start` and `repo_clone_chunk` but omitted
  `repo_clone_status` and `repo_clone_cancel`, even though both are
  re-exported from this module and registered in the `tools/call`
  dispatch table in `src/mcp/server.rs`. Added both to the rustdoc list
  with intra-doc links into `repo_clone_chunk` (where their handlers
  actually live).
- **`docs/ARCHITECTURE.md` "What Touches Disk" table understated Tier 2
  disk usage.** The table's `Tar archive | NO | Built in memory` row
  applied only to Tier 1 — Tier 2 sessions whose tar.gz is at least
  `DISK_THRESHOLD` (10 MiB, defined in `src/streaming/chunked.rs`) write
  the archive to a `NamedTempFile` so memory usage stays O(chunk size).
  Split the row into Tier 1 (always in memory) and Tier 2 (disk-backed
  when the threshold is exceeded, deleted when the session ends or
  expires). No code change.
- **`.claude/CLAUDE.md` source-tree comments still used the slash form
  for tool names** (`repo/clone`, `repo/push`, `repo/clone_start`, etc.)
  in the `mcp/tools/` block, even after the wider sweep that switched
  the rest of the docs to underscores in PR #141. Also added
  `tests/property_tests.rs` and `tests/security_tests.rs` to the
  `tests/` tree so it matches reality and the `docs/ARCHITECTURE.md`
  source-tree diagram.
- **Documentation accuracy sweep** — vague `**Response:**` summaries in
  `README.md` for `repo_clone`, `repo_push`, `repo_clone_start`,
  `repo_clone_chunk`, `repo_clone_status`, `repo_diff`, and `repo_refs`
  now name the actual response fields (matching the `RepoCloneResult`,
  `RepoPushResult`, etc. structs). The README configuration example now
  shows the `timeouts`, `limits`, and `rate_limits` sections — they were
  documented in the options table below but missing from the example
  block, which made the example look smaller than the real schema. Added
  a pointer to `config/example-config.json` for the fully-populated
  example. `CONTRIBUTING.md` PR Requirements checklist now lists
  `markdownlint-cli2` and the toolchain-pin pre-flight check (both run
  in CI). `docs/AI_WORKFLOW.md`'s `Initialize Response` heading is now
  `` `initialize` response `` — the protocol method name stays as the
  spec spells it but the surrounding prose is consistent with the
  British spelling used elsewhere. The `repo_clone`, `repo_push`, and
  `repo_pull` example responses in `AI_WORKFLOW.md` now show the actual
  `hint` strings the server returns instead of placeholder text
  (previously `"Bundle was successfully pushed to the remote."` and
  `"Apply the diff or extract files_archive into your local clone."`,
  neither of which the code has ever emitted). The `cargo clippy`
  example in the same doc now uses the strict
  `--all-targets --all-features -- -D warnings` form that CI runs, so
  the AI's local check matches what the project gates on.
  `CONTRIBUTING.md`'s `Types of Documentation` table now lists
  `STYLE.md` and every file under `docs/`, and the
  `Updating Documentation` list now spells out when to touch
  `docs/`, `STYLE.md`, and `config/example-config.json`.
- **`docs/SECURITY.md` had multiple fabricated APIs in its illustrative
  Rust snippets.** The file showed `RepoFilter::allowlist(vec![...])` and
  `RepoFilter::blocklist(vec![...])` constructors that have never
  existed (the real API is `RepoFilter::allowlist_mode()` /
  `blocklist_mode()` plus `.allow(pattern)` / `.block(pattern)` — see
  `src/security/guards.rs`); `PushGuard::new(allow_force_push: false)`
  and `RateLimiter::new(max_burst: 20, refill_per_sec: 5.0)` using
  Rust-doesn't-have-named-arguments syntax (and the latter also got the
  field name wrong — the parameter is `refill_rate`); a
  `push_guard.is_force_push(args)` method that doesn't exist (force
  pushes are detected inside `SecurityGuard::check`); a
  `sanitize_url(url)` function that doesn't exist (the real one is
  `sanitize_url_for_logging` and uses byte-safe `find` rather than the
  shown `Regex::new(...)` — the project has no regex dependency); and
  `audit_log.info(...)` / `audit_log.debug(...)` calls against an API
  that's never existed (audit events are constructed via
  `AuditEvent::repo_clone_success(...)` etc. and submitted via
  `AuditLogger::log_silent`). All snippets rewritten to compile against
  the real APIs. The same file's `log::info!` / `log::debug!` examples
  are now `tracing::info!` / `tracing::debug!` — this project depends on
  `tracing`, not `log`. Three "Unauthorized" / "unauthorized" instances
  in narrative text changed to British "Unauthorised" / "unauthorised"
  (the only remaining `Unauthorized` is the literal HTTP `401
  Unauthorized` reason phrase in `docs/errors.md`, which is part of the
  HTTP standard).
- **`README.md` Prerequisites omitted the runtime Git CLI requirement.**
  The server shells out to `git credential fill` (in
  `src/git2_ops/auth.rs`) and `git bundle unbundle` (in
  `src/git2_ops/push.rs`); a user installing only the prebuilt binary
  without git on `PATH` would have hit the credential helper failing to
  return anything and `repo_push` failing to apply the bundle. Added a
  `#### Git CLI` subsection explaining the requirement.
- **`.github/PULL_REQUEST_TEMPLATE.md` Code Quality checklist drifted
  from `CONTRIBUTING.md` § Pull Requests.** Added the same
  `markdownlint-cli2` and toolchain-pin pre-flight items, and tightened
  `cargo fmt` to the strict `cargo fmt --all --check` form CI runs.
- **`helper_script` Python helper's `show_info` enumerated `old_commit`,
  a key no MCP tool has ever returned.** The actual fields are
  `base_commit` + `new_commit` (`repo_pull`) and `base_commit` +
  `head_commit` (`repo_diff`). Replaced `old_commit` with `base_commit`
  and `head_commit`, and added a regression test
  (`helper_script_show_info_uses_real_commit_field_names`).
- **Inconsistent and imprecise size units across rustdoc and the MCP
  tool schema.** `src/streaming/chunked.rs` and
  `src/mcp/tools/repo_clone_start.rs` advertised the chunk and disk
  thresholds as `1MB` / `4MB` / `10MB`, but the constants are binary
  (`1024 * 1024`, `4 * 1024 * 1024`, `10 * 1024 * 1024`) — i.e. mebibytes,
  not megabytes. `src/config/settings.rs` already used `MiB`. Normalised
  every reference (rustdoc, the `tools/list` schema description for
  `chunk_size`, the README repo_clone_chunk response summary, and the
  ARCHITECTURE.md "Tier 2" trade-off table) to use `KiB` / `MiB`. The
  schema description now also reports the actual clamped range (1 KiB
  to 4 MiB) instead of just the maximum, and the `chunked.rs` module
  doc-comment now notes that the 1-hour session timeout is the *default*
  rather than a hard-coded value (the real timeout comes from
  `sessions.timeout_secs`).
- **`README.md` `submodule_depth` description omitted the `0` and `1`
  values.** The `tools/list` schema explicitly documents `1 = top-level
  only` and `0 = skip submodules entirely`, but the README only
  mentioned the unlimited default. Brought the README in line with the
  schema so callers can pick the right depth without reading the
  source.
- **Documentation lines exceeding the 170-character ceiling.**
  `CONTRIBUTING.md` line 78, and `STYLE.md` lines 207 and 283 had been
  173 / 192 / 174 columns respectively. Markdownlint was happy because
  MD013 ignores tables and code blocks, but the project's
  `.editorconfig` declares `max_line_length = 170` for all files.
  Wrapped them.
- **`protected_branches` default was documented imprecisely.** Three
  pieces of documentation (`docs/errors.md`, `README.md` configuration
  table, `BranchGuard::with_defaults` rustdoc) each described the
  default protected-branch set differently — and at least one was wrong
  outright. The actual logic is two-layered: `SecurityConfig::default()`
  sets `protected_branches` to an empty list, but `McpServer::new`
  treats an empty list as "use the built-in safe set" and substitutes
  `BranchGuard::with_defaults()`, which contains `main`, `master`, and
  `develop` (not `main`, `master`, `develop`, *and* `release/*` as the
  rustdoc had been claiming). Fixed all three: `docs/errors.md` and the
  README table now describe the two-layer behaviour and the resulting
  effective default; the `with_defaults` rustdoc now lists exactly what
  the function returns and clarifies that wildcard patterns are
  *supported* by the matcher but not part of the built-in set.
- **`src/lib.rs` Tier 2 tools list omitted `repo_clone_status`.** Like
  the earlier `tools/mod.rs` fix, the crate-level rustdoc named only
  three of the four Tier 2 tools. Added `repo_clone_status` and a new
  `Other tools` section listing the four operations that don't fit the
  Tier 1 / Tier 2 split (`repo_pull`, `repo_diff`, `repo_refs`,
  `helper_script`) so `cargo doc` no longer hides them.
- **`README.md` configuration table omitted defaults for half the
  options.** `git_identity.{name,email}`, `security.{protected_branches,
  repo_allowlist, repo_blocklist}`, `logging.{level, audit_log_path}`,
  `proxy.{url, no_proxy}`, `lfs.{max_object_size, max_total_size}`, and
  `submodules.{include_patterns, exclude_patterns}` had no default
  documented. Added the actual defaults from `src/config/settings.rs`
  (mostly `null`/empty/`warn`) so users no longer have to guess what
  happens when they leave a section out.
- **`helper_script` rustdoc didn't mention the `info` subcommand or the
  optional positional arguments.** The Python script supports `extract
  <result.json> [output_dir]`, `bundle <repo_dir> <since_commit>
  [head_ref] [output_file]`, and `info <result.json>`, but the
  module-level rustdoc only listed `extract <result.json> <output_dir>`
  and `bundle <repo_dir> <since_commit>` and presented the optional
  arguments as required. Updated the rustdoc and the `usage` string
  returned by `handle_helper_script` to use `[brackets]` for optionals
  and to include the `info` subcommand.
- **British-spelling sweep across `src/`.** The project's rule (see
  `CONTRIBUTING.md` § British Spelling) explicitly covers comments and
  user-facing strings, but several rustdoc comments, inline `//` notes,
  one log message, and one `thiserror`-derived `Display` string still
  used American spellings ("sanitize", "initialize", "initialization",
  "finalized"). Files touched: `src/session.rs`, `src/streaming/chunked.rs`,
  `src/streaming/tar.rs`, `src/git2_ops/{auth,clone,error,push}.rs`,
  `src/mcp/server.rs`, `src/mcp/tools/{mod,repo_push}.rs`. The user-facing
  change is the `Git2Error::InitFailed` `Display` string, which now reads
  `failed to initialise repository` instead of
  `failed to initialize repository`; the unit test that asserted on the
  literal string was updated to match. JSON-RPC method names
  (`initialize`, `notifications/initialized`) and reqwest's
  `StatusCode::UNAUTHORIZED` enum variant are spec/upstream identifiers
  and stay American — backticked in narrative text to make that clear.
- **`docs/SECURITY.md` audit-log snippet had the wrong arity** — the
  pass-3 rewrite of the audit example called
  `AuditEvent::repo_clone_success` with three arguments, but the real
  constructor takes six (`url, branch, commit, file_count, archive_size,
  duration`). Corrected, plus a note that the URL must be sanitised by
  the caller (the constructor does not).
- **`docs/AI_WORKFLOW.md` "Check Before Push" snippet still had
  `cargo fmt --check`** while CONTRIBUTING.md, STYLE.md, the PR
  template, and the rest of `AI_WORKFLOW.md` all use the strict
  `cargo fmt --all --check` form CI runs. Aligned.
- **`docs/AI_WORKFLOW.md` Step 1 (Clone) example never configured the
  Git identity** before its `git commit -m "Initial clone …"` call,
  even though the section above the example tells AI assistants to
  apply the `gitIdentity` from the `initialize` response so commits
  are clearly attributable. The example now includes the two
  `git config user.name` / `user.email` calls between `git init` and
  the first `git commit`.
- **`tests/integration/test_mcp_tools.py` module docstring described
  the wrong fixture.** The docstring still claimed the fixture had
  "2 commits, 2 tags, 5 files" — the layout from before PR #112's
  expanded-fixture commit. The real fixture (built by
  `tests/integration/rebuild_fixture.py`) has 5 commits, 2 tags, ~45
  source files plus a submodule, a renamed file, and an LFS-tracked
  binary. Replaced with the current layout and a pointer to
  `rebuild_fixture.py` as the canonical source.
- **`src/security/audit.rs` module-level rustdoc Log Format list was
  almost half empty.** It enumerated 8 of the 14 fields on `AuditEvent`,
  omitting `exit_code`, `shutdown_reason`, `url`, `branch`, `commit`,
  `file_count`, and `archive_size`. Replaced with a complete list and
  noted which event types each optional field applies to, plus the
  full set of `event_type` values (the previous list ended with "etc.").
- **`src/git2_ops/lfs.rs` security claim was too strong.** The module
  rustdoc said "All LFS server communication is over HTTPS", but
  `derive_lfs_url` happily accepts `http://` repo URLs and forwards
  the same scheme to the LFS endpoint (the `derive_lfs_url_http` test
  pins this behaviour). Reworded to say HTTPS for `https://` and `git@`
  URLs, HTTP for `http://`, and that `lfs+ssh://` is not supported.
- **`.github/ISSUE_TEMPLATE/bug_report.md` example version was a
  pre-1.0 placeholder.** The `git-proxy-mcp version: [e.g., 0.1.0]` hint
  predated the project's 1.0 release. Replaced with a pointer to
  `git-proxy-mcp --version` and a current-tier example (1.1.0).
- **`CONTRIBUTING.md` British-spelling reference table was a partial
  list.** It had 7 American/British pairs, missing the ones that came
  up repeatedly during this audit (`sanitize`, `authorize`, `finalize`,
  `recognize`, `customize`). Added them, plus a clarifying note that
  the JSON-RPC method names `initialize` and `notifications/initialized`
  are spec identifiers and stay American — backticked in narrative
  text.
- **`docs/AI_WORKFLOW.md` "Use Shallow Clone for Large Repos" tip
  recommended `repo_clone` for monorepos** without mentioning that
  truly large repos won't fit in a single MCP response and need
  Tier 2 (`repo_clone_start` + `repo_clone_chunk`). Reframed: shallow +
  sparse for medium repos, Tier 2 for everything that doesn't fit.
- **`helper_script.py` top-of-file docstring usage line for `bundle`
  was missing `[head_ref]`.** The runtime `Usage:` string printed when
  the user runs `bundle` with too few arguments correctly shows
  `bundle <repo_dir> <since_commit> [head_ref] [output_file]`, and
  `create_bundle` accepts both optional arguments, but the module
  docstring at the top of the script only listed `[output_file]` —
  inconsistent. Added `[head_ref]` so the module docstring matches the
  runtime help and the `Examples:` block (which already showed
  `bundle ./my-repo abc123def HEAD`).
- **`helper_script.py` `show_info` reported `archive_b64_length` for
  `repo_clone` results but no equivalent size for `repo_pull`.** The
  function now also reports `files_archive_b64_length` when a
  `files_archive` field is present, so `info` output is symmetric
  between the two result shapes.
- **`tests/property_tests.rs` module docstring, `.claude/CLAUDE.md`,
  and `docs/ARCHITECTURE.md`'s source-tree comment all claimed
  property tests covered `.gitmodules` parsing — they don't.** The
  file actually has 11 properties across URL sanitisation (3), URL
  validation (4), and LFS pointer detection/parsing (4). `.gitmodules`
  parsing has unit-test coverage in `src/git2_ops/submodule.rs::tests`,
  but no proptest. All three docstrings now describe the actual
  coverage.
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
