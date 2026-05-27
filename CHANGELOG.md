# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **`streaming::tar` coverage for previously-untested branches.** New unit
  tests drive the file-progress callback (`ProgressSender` configured), the
  `submodule_depth == 0` early skip, and both LFS-client-setup fallbacks
  (`resolve_lfs` with no `repo_url`, and with a `repo_url` whose scheme
  `derive_lfs_url` rejects) — none of which need network access. The long-path
  regression test is noted under Fixed.
- **`git2_ops::submodule` orchestration coverage.** The recursive fetch
  orchestration (`fetch_all_submodules` / `fetch_submodules_recursive` /
  `fetch_submodule`) was only exercised by the Python integration tests. Added
  unit tests that drive it without slow network: the no-`.gitmodules`,
  empty-`.gitmodules`, all-entries-filtered and `max_failures == 0` early-exit
  paths, and an eligible submodule whose URL refuses instantly (`127.0.0.1:1`)
  to cover the parallel-fetch batch and the failure-handling arm. Plus smaller
  gaps: an invalid exclude pattern, an unknown `.gitmodules` key, a section
  header without a closing quote, a nested-path submodule, and `.gitmodules`
  present as a tree rather than a blob. `submodule.rs` line coverage rose from
  77.17 % to 94.06 % (the remaining lines are the successful-fetch + child
  recursion arm, which needs a working remote and stays integration-covered).
- **`git2_ops::pull` fetch/delta-path coverage.** The body of `pull_changes`
  after the fetch (commit resolution, up-to-date check, the change-type
  classification loop, diff formatting, tar archiving) was untested. Extracted a
  private `pull_changes_inner` past `validate_url` and drove it against a local
  `file://` remote: a multi-change pull (added/modified/deleted + a detected
  rename, with stats and `files_archive`), the up-to-date short-circuit, and the
  invalid/absent `since_commit` error paths. `pull.rs` line coverage rose from
  53.35 % to 91.97 %.
- **`git2_ops::diff` fetch/diff-path coverage.** The body of `generate_diff`
  after the network fetch (commit resolution, tree diff, stats, patch
  formatting) was untested. Extracted a private `generate_diff_inner` past
  `validate_url` and drove it against a local `file://` remote with two commits:
  the full diff (modified + added file, stats, full base/head SHAs), tag-ref
  resolution, a missing-commit error, and a nonexistent-remote error. `diff.rs`
  line coverage rose from 70.56 % to 90.88 %.
- **`git2_ops::clone` fetch-path coverage.** `fetch_bare`'s body was untested
  (only `decode_default_branch` and the `validate_url` rejection were covered).
  Extracted a private `fetch_bare_inner` past the URL validation so tests drive
  the connect/fetch path against a local `file://` remote (`fetch_bare` still
  rejects `file://` on the public path): default-branch fetch, explicit-branch
  fetch, depth, proxy, missing-branch and nonexistent-remote errors, plus
  full-path `validate_url` rejection and an unreachable-host (`127.0.0.1:1`)
  test. `clone.rs` line coverage rose from 51.50 % to 93.57 %.

- **9 new `config` regression tests** taking `config/mod.rs` line
  coverage from 91.40 % to 97.97 % and adding drift guards around the
  configuration schema. `config/settings.rs` was already at 100 %; the
  additions there are correctness guards, not coverage:
    - `load_config_with_directory_path_returns_read_error` — exercises
      the previously-uncovered `ReadError` arm. A directory passes the
      `Path::exists()` check but cannot be read as a file, so it must
      surface as `ReadError`, not `NotFound`.
    - `load_config_none_resolves_default_path` — covers the `None`-path
      fallback to the platform default location. It asserts the single
      environment-independent invariant (the resolved default path is a
      file or absent, never a directory), so the test itself adds no
      environment-dependent uncovered arm.
    - `load_config_parses_shipped_example_config` — asserts the shipped
      `config/example-config.json` parses against the current `Config`
      struct, and that every default-valued section matches the code
      default. Because each section uses `deny_unknown_fields`, this
      catches drift in either direction (a field added to or removed
      from the struct or the example, or a default value that changed in
      only one place).
    - `session_config_defaults`, `parse_session_config`,
      `parse_session_config_partial` — `SessionConfig` previously had
      neither a defaults nor a parse test (every sibling config did);
      these also pin `SessionConfig::timeout()`.
    - `parse_lfs_config_timeout_fields` — pins the parse path of the
      three LFS HTTP-timeout fields, which the existing `parse_lfs_config`
      test does not set.
    - `rejects_removed_lfs_max_total_size_field` and
      `rejects_unknown_fields_in_every_subsection` — confirm
      `deny_unknown_fields` rejects unknown keys in every sub-struct, not
      just at the top level. The former guards the removed
      `lfs.max_total_size` key specifically.
  The only code left uncovered in `config/mod.rs` is the two-line
  `ok_or_else` closure that builds the `NotFound` error when
  `default_config_path()` returns `None` — reachable only when
  `dirs::home_dir()` returns `None`, which is not portably forceable in
  a test.
- **11 new `git2_ops::push::tests` regressions** taking `push.rs` line
  coverage from 34.75 % to 93.04 % (+58.29 pts). The file was the
  worst-covered in scope; only `push_options_default_no_force` and
  the existing `unbundle_sanitises_git_stderr_in_error` reached more
  than the type definitions. New tests:
    - `push_options_force_flag_round_trips`,
      `push_result_fields_are_accessible` — exercise the public
      types' Clone derives and field shapes.
    - `push_bundle_rejects_invalid_url`,
      `push_bundle_rejects_ext_url` — confirm `validate_url` rejects
      `file://` and `ext::` early, before any temp-dir or bundle
      work happens.
    - `push_bundle_fails_with_malformed_bundle_data` — covers the
      validate → temp-dir → init-bare → write → unbundle-fails
      chain on gibberish bundle bytes.
    - `push_bundle_returns_ref_not_found_when_branch_missing_after_unbundle`
      — produces a real bundle on `main` via `git bundle create`,
      asks `push_bundle` to push `feature/missing`, expects
      `RefNotFound("feature/missing")`.
    - `push_bundle_fails_when_remote_unreachable_after_successful_unbundle`
      — full happy path through unbundle + ref resolution + the
      push call itself, with the push failing on `127.0.0.1:1`
      (TCP RST'd immediately, no slow timeout).
    - `push_bundle_force_path_takes_force_refspec` — same as above
      with `force=true`, exercising the `+refs/heads/...` refspec
      branch in `push_to_remote` that the non-force test doesn't
      reach.
    - `push_bundle_uses_proxy_when_configured` — confirms that
      passing a proxy URL through doesn't break the call path
      (full proxy traffic verification needs a real proxy and is
      out of scope for unit tests).
    - `unbundle_succeeds_with_valid_bundle` — covers the success
      arms of `unbundle` by producing a real bundle, unbundling
      into a fresh bare repo, and asserting `refs/heads/main`
      now resolves to the seeded commit.
    - `unbundle_returns_bundle_failed_when_bundle_path_missing` —
      a distinct "git ran but exited non-zero" trigger from the
      existing malformed-bundle test, hitting the same sanitisation
      path with a path-not-found stderr.
- **Three new `LfsConfig` HTTP-timeout fields** added with
  `#[serde(default)]`, so existing configs continue to parse unchanged:
    1. `request_timeout_secs` (default 300) — caps the LFS Batch API
       POST. Set as the `Client::builder().timeout` default.
    2. `connect_timeout_secs` (default 30) — caps TCP+TLS handshake.
    3. `download_timeout_secs` (default 600) — per-object download GET,
       typically larger than `request_timeout_secs` because object
       downloads can be much slower than the Batch API call. Applied
       via `RequestBuilder::timeout`, overriding the Client default
       for the GET only.
  Documented in `config/example-config.json` and the README config
  table.
- **21 new `git2_ops::lfs::tests` regressions** covering the LFS audit
  (PR #159) fixes — every fix lands with the test that would have caught
  it, plus a Codecov-driven follow-up batch that pushed `lfs.rs` line
  coverage from 91.87 % to 98.42 %:
    - Fix-coverage tests:
      `is_lfs_pointer_does_not_match_hypothetical_future_v10`,
      `is_lfs_pointer_accepts_crlf_line_ending`,
      `derive_lfs_url_rejects_ssh_with_empty_host`,
      `derive_lfs_url_rejects_ssh_without_colon`,
      `derive_lfs_url_rejects_ssh_with_empty_path`,
      `derive_lfs_url_rejects_https_with_empty_host`,
      `fetch_content_rejects_oversize_actual_response`,
      `fetch_content_sanitises_server_error_body_in_log`,
      `fetch_content_sanitises_lfs_error_message`,
      `user_agent_contains_crate_version`.
    - Codecov-gap tests:
      `parse_lfs_pointer_skips_blank_lines`,
      `lfs_client_constructs_with_proxy_and_no_proxy`,
      `lfs_client_constructs_with_proxy_only`,
      `lfs_client_rejects_invalid_proxy_url`,
      `fetch_content_retries_download_get_on_transient_5xx_then_succeeds`
      (covers the *download* GET retry loop, distinct from the
      Batch-API POST retry the existing test exercises),
      `fetch_content_emits_progress_when_sender_configured`
      (exercises the chunked-read progress callback),
      `fetch_content_passes_through_server_supplied_download_headers`
      (exercises the per-object header forwarding loop),
      `fetch_content_warns_but_returns_when_actual_size_under_declared`,
      `fetch_content_does_not_retry_download_get_on_4xx`,
      `fetch_content_returns_connect_error_when_download_host_unreachable`
      (exercises `is_transient_error` + the retry-then-give-up path
      via 127.0.0.1:1 unreachable port),
      `fetch_content_returns_connect_error_when_lfs_host_unreachable`
      (same connection-error path applied to the Batch-API POST).
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

- **`streaming::tar` test and doc hygiene** (no behaviour change):
    1. `is_binary`'s doc-comment claimed its heuristic is "similar to what Git
       uses internally" — Git's core check is only NUL-in-first-8000-bytes; the
       30%-non-printable rule is this crate's own and (because UTF-8 multibyte
       bytes are ≥ 0x80) can misclassify mostly-non-Latin text as binary. The
       doc now states this accurately.
    2. `is_binary_exactly_at_threshold` asserted nothing (`let _ = result;`); it
       now asserts the real boundary behaviour (exactly 30% non-text is treated
       as text, since the check is `> threshold`, not `>=`).
    3. `is_binary_accepts_utf8` computed an unused `_utf8` binding; removed.
    4. `tar_options_default` was fully subsumed by
       `tar_options_default_has_all_none`; removed the duplicate.
- **`Config::validate` now range-checks the configuration instead of being a
  no-op.** Previously every value that parsed was accepted; a handful of
  out-of-range values then silently broke a subsystem — or panicked. `validate`
  (run by `load_config` at startup) now rejects, with a `ValidationError` that
  names the offending field:
    1. **Zero timeouts** — `timeouts.request_timeout_secs` and the three
       `lfs.*_timeout_secs`. `Duration::from_secs(0)` makes the corresponding
       git command or LFS HTTP request fail immediately.
    2. **`rate_limits.max_burst` of zero** — the token bucket starts empty and
       never refills past zero, so *every* operation is blocked forever.
    3. **A non-finite or negative `rate_limits.refill_rate_per_sec`** — this is
       the one that was not merely a foot-gun: `NaN` reaches
       `RateLimiter::time_until_available`, whose `Duration::from_secs_f64`
       **panics** on a non-finite value. The infinities and negatives don't
       panic but break the token-bucket maths (permanent block, or effectively
       no throttling). `0.0` is still accepted (the supported "burst once,
       never refill" mode).
    4. **Zero session limits** — `sessions.timeout_secs` (sessions expire
       instantly), `sessions.max_streaming_sessions` and
       `sessions.max_repo_sessions` (no session can ever be created).
    5. **An unrecognised `logging.level`** — previously any unknown string
       silently became `warn`; a mistake such as `"verbose"` or `"warning"`
       (the level is `warn`, not `warning`) is now rejected with the list of
       valid levels.
  Values that the consuming code already handles are deliberately *not*
  rejected: `submodules.max_concurrent` (clamped to ≥ 1 by the fetcher),
  `submodules.max_failures` of 0, `lfs.retry_max_attempts` of 0 (the retry loop
  always makes one attempt), and `lfs.max_object_size` of 0 (every object kept
  as a pointer). Eight new unit tests cover every rejected and every
  deliberately-accepted case, and confirm `load_config` surfaces the
  `ValidationError` (i.e. validation is wired into the load path). `validate` is
  no longer `const fn` (it now builds error strings); no other API change.
- **`git2` requirement bumped 0.20.4 → 0.21.0** (cargo-dependencies group).
  0.21 ships two breaking changes that touched this crate:
    1. **`default` features are now empty** (0.20 enabled `ssh` + `https`).
       `Cargo.toml` now requests `features = ["https", "ssh"]` explicitly —
       the exact set 0.20.4 enabled by default — which also pulls in the new
       `cred` feature that gates `Cred::credential_helper`. No change to the
       credential or transport behaviour.
    2. **`TreeEntry::name()` and `Commit::message()` now return
       `Result<&str, Error>`** (UTF-8 validation) instead of `Option<&str>`.
       The three `let Some(name) = entry.name()` walk callbacks in
       `streaming/tar.rs` and `git2_ops/submodule.rs` became
       `let Ok(name) = …`, preserving the "skip non-UTF-8 names" behaviour;
       one `push.rs` test assertion moved from `Some(..)` to `Ok(..)`.
  Also replaced the now-deprecated `Oid::zero()` with the `Oid::ZERO_SHA1`
  constant in three `submodule.rs` tests. No production behaviour change.
  Three new regression tests close the unit-coverage gap on the two
  migrated submodule walk callbacks (previously only exercised by the
  Python integration tests, which don't run in the PR coverage job):
  `find_submodule_entries_detects_gitlink_in_tree` and
  `find_submodule_entries_gitlink_missing_from_gitmodules_is_skipped`
  build a tree with a real gitlink (mode 160000) entry, and
  `write_submodules_to_tar_writes_submodule_files` drives
  `write_submodules_to_tar` against a locally-created bare repo via a new
  test-only `FetchResult::from_parts_for_test` constructor (no network).
- **`download_with_retry` and `download_with_progress` collapsed into a
  single `download_chunked` helper.** The two had drifted on observability
  (the progress variant was missing `url` in its `warn!` calls) and the
  duplication meant any future retry-logic change had to be made in two
  places. The unified path always reads in 64 KiB chunks (so the new
  actual-size cap applies regardless of whether progress is enabled), and
  emits progress only when a `ProgressSender` is configured. The retry
  loop was further split into `download_chunked` (request + retry) and
  `read_response_body` (chunked read + cap + progress) so the cap-and-cap
  logic lives in one place.
- **LFS user-agent now derived from `CARGO_PKG_VERSION`.** The previous
  `"git-proxy-mcp/0.1"` was hardcoded and had drifted from the actual
  crate version (now 1.1.0) over the v1 development cycle. The new const
  `USER_AGENT = concat!("git-proxy-mcp/", env!("CARGO_PKG_VERSION"))`
  stays in lockstep with `Cargo.toml` automatically. A regression test
  (`user_agent_contains_crate_version`) enforces the contract.
- **Disk-backed `StreamingSession` storage path now has direct test
  coverage.** All previous chunked-session tests used data well below
  `DISK_THRESHOLD` (10 MiB), so the `SessionStorage::File` variant —
  including `seek`, `read_exact`, and the `NamedTempFile` lifecycle
  — had zero direct coverage. Added
  `disk_backed_storage_round_trips_chunks_correctly` which forces
  the file path with a ~12 MiB allocation and asserts byte-exact
  round-trip across all chunks, the partial-tail last chunk, and
  `is_complete()` semantics. Test runs in ~110 ms locally.
- **`rand_u64` doc-comment corrected.** Previously claimed
  `thread_hash` (computed as the byte length of the thread-id's
  Debug repr) was an "entropy source" — but
  `format!("{:?}", ThreadId(N))` produces strings of length 11–12
  for typical thread IDs, contributing at most a few bits of
  variance. Real uniqueness comes from the `AtomicU64` counter and
  nanosecond timestamp. Updated the doc-comment to describe each
  source's actual contribution accurately, plus an explicit threat
  model note ("Not cryptographically secure" — single MCP process,
  no cross-client attack vector). No code change.
- **`sanitize_for_log` extracted to a shared `src/util.rs` module.**
  Previously a private helper in `mcp/server.rs` (added in PR #154 for
  client-controlled `clientInfo` fields), the same hardening pattern
  is now needed in `git2_ops/push.rs::unbundle` for git's stderr.
  Rather than duplicate, exported via `pub mod util` with `pub fn
  sanitize_for_log`. New module is small: one helper, one constant,
  7 tests. No behaviour change.
- **Removed dead `parse_bundle_info` + `BundleInfo` + `BundleRef`
  from `streaming/bundle.rs`.** The function was exported and tested
  but had no production callers — only its own 3 tests (2 unit + 1
  integration). It also had subtle issues (header truncated at 512
  bytes, ref-line detection trusts bundle content) that would have
  needed addressing if it were ever wired up. 95 lines removed; no
  production behaviour change. Bundle handling for `repo_push`
  continues to work through `decode_bundle` + `validate_bundle` +
  git's own unbundle parsing — all kept.
- **MCP server observability for the `initialize` handshake and
  ignored notifications.** Three previously-silent paths are now
  traced:
  1. **Connecting client identity is logged at INFO** —
     `client_name`, `client_version`, and the requested
     `client_protocol_version` from `clientInfo` (or a shorter "no
     clientInfo" line when the field is absent, which the spec
     allows). The handler had been parsing these fields into
     `_params` only to discard them.
  2. **Mismatched protocol version is logged at WARN** with both
     `requested` and `supported` versions. Previously the handler
     silently substituted our supported version with no trace,
     making client-version drift impossible to diagnose from server
     logs. Behaviour unchanged — we still accept and respond with
     our version per spec, just with visibility now.
  3. **Ignored notifications are traced at DEBUG** with method name
     and current server state. Useful when the client thinks it sent
     something we should have acted on (typically a state-machine
     misalignment, e.g. sending a notification before `initialize`).
  Two new tests cover the previously-uncovered initialize paths
  (`handle_initialize_accepts_mismatched_protocol_version` and
  `handle_initialize_accepts_missing_client_info`).
- **`get_credentials_for_url` now traces each subprocess failure mode
  at debug level.** Previously, every error path
  (`spawn` fail, stdin write fail, wait fail, non-zero exit, non-UTF-8
  stdout, missing `username`/`password` in helper output) was collapsed
  silently to `None` via `.ok().and_then(...)?`. The caller (LFS)
  reported "LFS client created without credentials" without the user
  being able to tell whether `git` was missing from PATH, no credential
  helper was configured, or the helper ran and returned nothing useful.
  Now `RUST_LOG=debug` makes "no credentials" diagnosable from the
  logs alone. No change to the function's `Option<(String, String)>`
  return shape.
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

- **`create_tar_from_tree` silently dropped files whose path was too long for
  a tar header.** The tar builder set the entry path via `tar::Header::set_path`
  and then `append`, but `set_path` fails for any path that doesn't fit the
  ustar 100-byte `name` field (and can't be split across the 155-byte `prefix`
  at a `/`) — e.g. a single path component longer than 100 bytes. Such files
  were counted in `skipped_path_too_long` and left out of the archive, so a
  repository with deep paths (common in `node_modules`, generated code, or
  deeply nested packages) reached the AI missing files with no error reported.
  Both the main-tree walk and the submodule walk now use
  `tar::Builder::append_data`, which emits a GNU long-name (`././@LongLink`)
  entry for over-long paths — the same approach
  `git2_ops::pull::create_files_archive` already used. `skipped_path_too_long`
  now only counts paths that cannot be encoded at all (e.g. an embedded NUL,
  which a git tree name can never contain). Regression-tested by
  `create_tar_includes_file_with_long_path`, which archives a 154-character
  filename and reads it back out of the resulting tar.
- **Submodule progress never reached 100%.** `write_submodules_to_tar` advanced
  its `processed_submodules` counter only on the "added files" and "walk failed"
  arms, so a submodule that walked successfully but contributed no files (empty,
  or fully removed by the sparse/binary/size filters) advanced neither it nor a
  result counter — leaving the submodule progress percentage permanently short.
  The counter is now incremented once per submodule regardless of outcome;
  `submodules_included` / `submodules_failed` semantics are unchanged.
- **`repo_clone` / `repo_clone_start` never surfaced the
  `skipped_path_too_long` counter.** Every other skip counter
  (`skipped_by_filter`, `skipped_binary`, `skipped_too_large`) is included in
  the tool response when non-zero, but `skipped_path_too_long` was tracked in
  `TarResult` and then dropped on the way out — so before the long-path fix
  above, a file with an over-long path was silently omitted *and* the count
  that would have revealed it never reached the client. `RepoCloneResult` and
  `RepoCloneStartResult` now carry the field (same
  `skip_serializing_if = "is_zero"` treatment as its siblings), and it is
  listed in the README response fields.
- **`repo_pull` never detected renames, so a renamed file was reported as a
  delete + an add with no `old_path`.** `pull_changes` ran `diff_tree_to_tree`
  but never called `Diff::find_similar`, which is what actually coalesces a
  delete/add pair into a rename — so the `Renamed` match arm and
  `ChangedFile::old_path` were dead, and `stats.files_changed` double-counted a
  rename as two files. `find_similar` is now run before the deltas and stats are
  read, so renames (and copies) are reported with their old path and counted
  once. Regression-tested by
  `pull_changes_inner_reports_changes_and_detects_rename`.
- **`repo_diff` could not resolve a short (abbreviated) commit SHA** despite
  documenting "Short SHA (minimum 4 hex chars)" support. `resolve_commit` tried
  `Oid::from_str` first and returned on success — but `Oid::from_str` *zero-pads*
  a short hex string into a bogus full OID (e.g. `617b10e692` →
  `617b10e692000…000`) rather than resolving the abbreviation, so it never
  reached the `revparse_single` call that actually resolves short SHAs against
  the repo. `find_commit` then failed with a misleading "commit not found". The
  direct-OID fast path is now taken only for a full 40-char SHA; anything shorter
  (and branch/tag/`HEAD~N` refs) goes through `revparse_single`. Regression-tested
  by `resolve_commit_short_sha`.
- **`repo_refs` reported a non-deterministic default branch when several
  branches shared the `HEAD` commit.** `list_remote_refs` derived
  `default_branch` by scanning the branch list for the first entry whose OID
  equalled `HEAD`'s OID — but that scan ran over the remote's *advertised* ref
  order (before the alphabetical sort), so a repo where, say, `develop` and
  `main` both point at the same commit (a freshly-cut branch off `main`) could
  report whichever the remote happened to list first. It now uses the remote's
  advertised `HEAD` symref via `Remote::default_branch()` (the same
  authoritative source `clone.rs`'s `fetch_bare` already relies on), falling
  back to `"main"` only when no symref is advertised. The OID-matching logic is
  gone. As part of the fix, the connect/list/parse body was split into a private
  `list_refs_inner` helper so the path can be exercised against a local
  `file://` remote (`list_remote_refs` still rejects `file://` via
  `validate_url` before delegating), and the default-branch resolution is now a
  pure `resolve_default_branch` helper. `git2_ops/refs.rs` line coverage rose
  from 49.53 % to 99.04 % (15 tests, up from 4): a local-remote integration test
  (branches, lightweight + annotated tags with `^{}` peeled entries skipped,
  HEAD excluded, the same-commit ambiguity that the bug mishandled), seven
  `resolve_default_branch` cases, and proxy/empty-repo/unreachable-remote paths.
- **`is_lfs_pointer` would have misclassified a hypothetical future
  `spec/v10` (or `v11`, `v100`, …) as a v1 pointer.** The previous
  check was `starts_with("version https://git-lfs.github.com/spec/v1")`,
  and `v1` is a prefix of `v10` — so a future spec bump of git-lfs
  would silently get treated as the current format. Now matches the
  whole first line via `text.lines().next() == Some(LFS_POINTER_VERSION_LINE)`,
  which also handles CRLF cleanly (regression-tested for both
  `is_lfs_pointer_does_not_match_hypothetical_future_v10` and
  `is_lfs_pointer_accepts_crlf_line_ending`). No known production
  trigger today, since v1 is still the only spec — but the same
  bug class hit `auth.rs` in PR #153 (empty-host SSH URLs).
- **`derive_lfs_url` accepted malformed SSH and HTTP(S) URLs and
  silently produced useless requests.** `git@:repo.git` rewrote to
  `https:///repo.git` (no host); `git@host` (no `:`) rewrote to
  `https://host` (no path); `https:///x` was passed through verbatim.
  Each produced a request that would have failed at the LFS server
  with a confusing error and leaked the malformed URL into operator
  logs. Now validates that the SSH form has both a non-empty host
  and a non-empty path component, and that the HTTP(S) form has a
  non-empty host. Same bug class as the auth.rs empty-host SSH fix
  in PR #153.
- **`StreamingSession::get_chunk` could corrupt resume tracking on
  disk-read failure.** Two coupled bugs: (1) `retrieved_chunks[index]`
  was set to `true` BEFORE the storage read, so a failed read on
  disk-backed storage (truncated temp file, disk error during seek,
  permission loss) silently marked the chunk as retrieved even though
  the AI never received its data — `next_missing_chunk()` would skip
  the index forever, leaving the session unable to complete on retry.
  (2) The `Option<ChunkData>` return type conflated bounds errors and
  I/O errors, so a disk failure surfaced as
  `StreamingError::InvalidChunkIndex` in the MCP response — completely
  wrong diagnostic for the operator. Both bugs only affected disk-
  backed sessions (archives > 10 MiB) and were never triggered by
  existing tests, which all used in-memory storage.
  Fix: introduced `ChunkReadError` enum (`OutOfBounds` /
  `Io(io::Error)`) and reordered the body to read FIRST, mark
  retrieved only after success. Manager translates the new error
  variants to distinct `StreamingError` cases. New regression tests
  cover the bounds-vs-IO distinction and the
  out-of-bounds-doesn't-mark invariant. The `get_chunk` API on
  `StreamingSession` changes from `Option<ChunkData>` to
  `Result<ChunkData, ChunkReadError>` — internal to the streaming
  module; `StreamingSessionManager::get_chunk` external API is
  unchanged.
- **`unbundle` included git's raw stderr in error messages without
  sanitisation.** A maliciously-crafted bundle that triggered a
  creative `git fetch --no-tags <bundle>` failure could produce stderr
  with ANSI escape sequences, embedded newlines, or megabytes of
  output — all of which then flowed into both `tracing::warn!` log
  lines (operator's terminal) and the MCP response (returned to the
  AI client). Without sanitisation, a single error from
  `unbundle` could repaint the operator's terminal log reader, fake
  log-line boundaries (e.g. "fatal: early EOF\nerror: index-pack
  died\n" creates two log lines from one error), or flood the log
  file. Now passes git's stderr through `crate::util::sanitize_for_log`
  before formatting — same protection profile as PR #154's
  client-controlled JSON sanitisation.
- **Audit log misidentified blocked `repo_refs` / `repo_diff` /
  `repo_pull` operations as `repo_clone`.** `call_repo_refs_tool`,
  `call_repo_diff_tool`, and `call_repo_pull_tool` were all calling
  `AuditEvent::repo_clone_blocked(...)` for their rate-limit and
  filter-block events — so an operator searching the audit log for
  blocked refs / diff / pull operations would find nothing (they all
  showed as `event_type: "repo_clone"`). Added three new
  `AuditEventType` variants (`RepoRefs`, `RepoDiff`, `RepoPull`)
  and matching `repo_*_blocked` constructors, then updated the three
  tool functions to use them. Three new audit-module tests cover
  each constructor; one regression test pins that all three serialise
  with their proper `event_type` field.
- **Tier 2 `repo_clone_start` audit log always recorded
  `file_count: 0`.** `call_repo_clone_start_tool` passed `0` to
  `AuditEvent::repo_clone_success` with a comment claiming the count
  was "not known until all chunks retrieved". That hasn't been true
  since `RepoCloneStartResult` gained the `file_count` field — the
  archive (and hence the file count) is fully built before the first
  chunk goes out the door. Now passes `result.file_count`. Tier 1
  (`repo_clone`) was logging the real count all along, so the bug
  only affected Tier 2 audit entries.
- **JSON-RPC `parse_message` discarded request `id` on
  `InvalidRequest` errors when the rest of the request was malformed.**
  Per JSON-RPC 2.0 spec, when the `id` field is well-typed but other
  fields (jsonrpc version, method) are malformed, the error response
  SHOULD echo the `id` so the client can correlate the failure to its
  outstanding request. Three uncovered cases were dropping the ID:
  missing `jsonrpc` field, wrong `jsonrpc` version, and `jsonrpc`
  field of wrong type. Added a small `extract_request_id` helper that
  pre-walks the JSON object and returns `Some(RequestId)` only for
  spec-valid shapes (integer or string); `null`, arrays, objects, and
  non-integer numbers correctly map to `None`. Eight new tests cover
  ID-preservation across each discard case plus ID-drop for each
  invalid shape.
- **`sanitize_url_for_logging` mangled URLs with `@` outside the
  authority component.** The function found the FIRST `@` anywhere in
  the URL and treated everything between `://` and that `@` as
  userinfo, replacing it with `***`. So a URL like
  `https://github.com/owner/repo?email=foo@bar.com` rendered as
  `https://***@bar.com` — wrong host, missing path, missing query
  prefix. Same shape for `@` in path or fragment. Per RFC 3986, the
  userinfo `@` separator can only appear in the authority component
  (between `://` and the first `/` `?` or `#`), so the fix scans only
  that substring for `@`. Not a credential leak — the buggy code
  over-stripped, hiding info rather than exposing it — but it produced
  misleading log output that could mask which repository was being
  accessed during diagnostics, and a crafted URL could exploit it for
  minor log spoofing. Five new regression tests cover `@` in query /
  path / fragment, `@` in both userinfo AND path simultaneously, and
  authority with port.
- **`parse_url_for_credentials` accepted SSH URLs with empty host.**
  Inputs like `git@:path` or `git@` (no host) went through
  `split(':').next()` and returned `Some(("https", ""))`, which then
  drove `git credential fill` with `host=` — that either fails or,
  worse, accidentally matches a default-configured host the user
  didn't intend. Added a non-empty check before returning, plus two
  regression tests. A short docstring note also clarifies that only
  canonical `git@host:path` SSH URLs are recognised by that branch;
  `gitea@`, `gerrit@`, and `ssh://` URLs go through `Url::parse`.
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

### Removed

- **`LfsClient::fetch_batch` and the `LfsBatchResult` struct deleted.**
  Both were `pub` but had zero callers outside `lfs.rs`'s own tests
  (the only LFS consumer, `streaming/tar.rs`, calls `fetch_content` per
  blob). Per the dead-code cleanup pattern from PR #155 (which removed
  `parse_bundle_info` for the same reason), keeping a half-finished
  `pub` API around just means its bugs need fixing without ever being
  exercised in production. The `~140 LOC` of batch logic plus three
  associated tests are gone; the in-tree consumer is unaffected. If
  batch fetching is wired in later, the function can be revived from
  history with the audit-era hardening already applied.
- **`LfsConfig.max_total_size` removed.** Was only consumed by
  `fetch_batch`; with that gone, the field had no effect. Configs
  that previously set it must drop the key (LFS config still uses
  `deny_unknown_fields`). The per-object cap (`max_object_size`)
  remains and is now enforced both pre-flight against `pointer.size`
  and in-flight against the actual response body. If a per-operation
  total cap is needed, it should be tracked in the consumer
  (`streaming/tar.rs`) across `fetch_content` calls — a feature, not
  a regression of an unused field.

### Security

- **Network git2 error messages are now routed through a credential-safe
  sanitiser instead of being wrapped raw.** The connect/fetch/push call sites in
  `git2_ops::{clone,diff,pull,push}` mapped errors with
  `FetchFailed(e.message().to_string())` / `PushFailed(...)`, bypassing the
  sanitising `From<git2::Error>` impl. A git2 error that echoed a URL with
  embedded userinfo (e.g. `https://user:token@host`) would then reach the
  operator's logs and the MCP response unredacted. Two credential-safe
  constructors — `Git2Error::from_fetch` and `from_push` — now collapse
  auth-class errors to the detail-free `AuthenticationFailed` and run any other
  message through `sanitize_error_message`, which additionally redacts the
  userinfo of any `scheme://user:secret@host` substring (the existing
  keyword-line filter would miss a token embedded in a URL). The local-object
  error sites (tree/commit/diff/revwalk/peel) are intentionally unchanged — they
  cannot contain credentials. `From`'s behaviour is unchanged (refactored to
  share an `is_auth_error` helper). Regression-tested with `redact_url_userinfo`,
  `from_fetch`/`from_push`, and unreachable-host tests for clone/diff/pull.
- **LFS download size now bounded against the *actual* response body,
  not just the pre-flight pointer size.** The previous
  `pointer.size > max_object_size` check was necessary but not sufficient:
  the LFS server controls how many bytes it actually returns, and the
  old `read_to_end` accepted whatever came back. A malicious or buggy
  server could claim `size: 100` in the Batch API response and then
  return megabytes (or gigabytes) of data, making us allocate and hold
  arbitrary memory. The download path now reads in fixed 64 KiB chunks
  and bails as soon as the cumulative byte count exceeds
  `max_object_size` (configured per-deployment), capping memory growth
  at one chunk past the limit. Regression-tested by
  `fetch_content_rejects_oversize_actual_response`.
- **LFS pre-allocation capped at 16 MiB regardless of declared
  `pointer.size`.** A hostile pointer claiming `pointer.size = u64::MAX`
  would otherwise have crashed the process via `Vec::with_capacity(usize)`
  OOM before we'd even started reading. The `INITIAL_DOWNLOAD_CAPACITY_CAP`
  const lets the buffer grow on demand beyond 16 MiB, but the actual-byte
  cap above stops growth from running away.
- **HTTP timeouts now configured for the LFS client.** The
  `reqwest::blocking::Client::builder()` had no `.timeout()` or
  `.connect_timeout()`, so a hung LFS server (slowloris, half-open TCP,
  TLS handshake stalled) could pin the entire MCP operation
  indefinitely. New `lfs.request_timeout_secs` (default 300) and
  `lfs.connect_timeout_secs` (default 30) cap both, configurable per
  deployment in `config.json`.
- **LFS server-supplied strings now sanitised before logging or
  returning in error messages.** Three server-controlled strings flow
  through `crate::util::sanitize_for_log` (extracted to shared use in
  PR #155 for git stderr): the non-retryable Batch API response body,
  the per-object `error.message` field in the Batch API JSON, and any
  text surfaced via the chunked-download error path. Without this,
  a hostile or buggy LFS server could inject ANSI escape sequences
  (repaint the operator's terminal) or fake newlines (forge log-line
  boundaries) — same bug class fixed for `unbundle` git-stderr in
  PR #155 and `clientInfo` JSON-RPC fields in PR #154.
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
