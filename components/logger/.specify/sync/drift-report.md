---
spec_sync_component: logger
spec_sync_drift_status: clean
spec_sync_synced_at: 2026-09-03T23:38:13Z
spec_sync_git_commit: d608c9db
spec_sync_inputs_sha256: d90bc4a67da9a7ebf785ed1fb1b200233a048926b341ce6ee195eb0d6cf3ddca
spec_sync_hash_tool: scripts/spec-sync-hash.sh
---
# Drift Report: logger

**Generated**: 2026-09-03 (Spec-Sync re-sweep)
**Project**: logger
**Spec analyzed**: `specs/001-logger-component/spec.md` (Status: Draft, Last-Synced 2026-08-20)
**Mode**: Read-only drift analysis + freshness stamp. No code or spec change
required this sweep — the component is fully aligned.

This sweep supersedes the earlier stale artifact (which read "Generated: pending",
listed **1 Drifted** — FR-006 "yellow for warn" vs code orange — and 20 aligned).
That artifact **predated the 2026-08-20 Phase B spec update**, which BACKFILLED
FR-006 to name the exact escapes the implementation emits (proposals.md /
apply-report.md). The current spec text (FR-006, US1 AC-5) already specifies the
256-color orange warn escape `\x1b[38;5;208m`, so the reported drift no longer
exists: spec, code, and the passing unit test all agree on orange.

## Summary

| Metric | Count |
|--------|-------|
| Specs Analyzed | 1 (`001-logger-component`) |
| Requirements Checked | 14 FR (FR-001…014) + 7 SC (SC-001…007) = 21 |
| Aligned | 21 |
| Drifted | 0 |
| Not Implemented | 0 |
| Unspecced Features | 0 |

**Verification runs this sweep** (all green):
- `cargo build -p logger` — clean
- `cargo clippy -p logger --all-targets -- -D warnings` — clean
- `cargo test -p logger -- --test-threads 1` — 15 unit + 7 integration + 5 doc
  = 27 passed; 0 failed.

## Detailed Findings

### Spec 001-logger-component — Logger Component

**Aligned ✓** (verified this sweep against the implementation)
- FR-001 `LoggerComponent` via `define_component!` — `src/lib.rs:123-131`
- FR-002 `ILogger` defined in interfaces crate via `define_interface!`
  (error/warn/info/debug) — `../interfaces/src/ilogger.rs:3-10`; consumed via
  `impl ILogger for LoggerComponent` `src/lib.rs:222-238`
- FR-003 `IUnknown` discovery for `ILogger` — via `define_component!`
  (`src/lib.rs:123`); exercised by `test_query_interface_ilogger`,
  `test_query_interface_returns_some`, `test_provided_interfaces`
  (`tests/integration.rs:21-51`)
- FR-004 `RUST_LOG` filtering, env_logger semantics — `from_env_str`/`from_env`
  `src/lib.rs:57-73`; `log()` threshold check `src/lib.rs:200`
- FR-005 console/stderr default — `LoggerState::default` `src/lib.rs:110-121`
  (writer = `io::stderr()`)
- FR-006 ANSI colors per level — `ansi_color()` `src/lib.rs:84-91`: error red
  `\x1b[31m`, **warn orange `\x1b[38;5;208m`**, info green `\x1b[32m`, debug cyan
  `\x1b[36m` — matches the (2026-08-20 backfilled) spec exactly; asserted by
  `test_all_levels_colored` `src/lib.rs:378-389`
- FR-007 file output mode — `new_with_file` (append+create) `src/lib.rs:155-165`
- FR-008 file output no ANSI — `use_color: false` `src/lib.rs:163`
- FR-009 timestamp + level + message — `log()` `src/lib.rs:203-215` (RFC3339
  millis, 5-char padded level, message)
- FR-010 thread-safe (`Mutex` writer, no interleaving) — `src/lib.rs:105,216`;
  asserted by `test_concurrent_logging_no_interleave` (`tests/integration.rs:91`)
- FR-011 default level info when RUST_LOG unset — `from_env` `src/lib.rs:68-73`
  (`Err(_) => LogLevel::Info`)
- FR-012 `new_with_writer(Box<dyn Write+Send>, LogLevel, use_color)` —
  `src/lib.rs:187-197`
- FR-013 TTY auto-detect via `libc::isatty(STDERR_FILENO)` — `src/lib.rs:114`;
  file loggers force color off
- FR-014 public `LogLevel` (`from_env_str`, `Display`), public `LoggerState`
  wrapping writer/level/color — `src/lib.rs:36,57,94,104`
- SC-001 all four methods produce timestamp+level+message — unit format tests
  (`src/lib.rs`) + doc examples
- SC-002 level filtering suppresses below threshold — level-ordering + filtering
  unit tests
- SC-003 console distinct colors per level on a terminal — `test_all_levels_colored`
  (`src/lib.rs:378-389`)
- SC-004 file output zero ANSI — `test_file_output_no_ansi`
- SC-005 4+ concurrent threads, no interleaving — `test_concurrent_logging_no_interleave`
  (400 lines, `tests/integration.rs:91-125`)
- SC-006 `IUnknown` query for `ILogger` + binds to receptacles —
  `test_query_interface_*`, `test_receptacle_binding`, `test_receptacle_info`
  (`tests/integration.rs`)
- SC-007 doc tests + Criterion benchmarks — 5 doc-tests in `src/lib.rs`;
  `benches/log_throughput.rs` wired via `[[bench]]` in `Cargo.toml`

**Drifted ⚠️**
- None. (The earlier report's FR-006 "yellow vs orange" finding was resolved by
  the 2026-08-20 BACKFILL that reworded the spec to the implemented orange escape.)

**Not Implemented ✗**
- None.

## Unspecced Features

| Feature | Location | Lines | Suggested Spec |
|---------|----------|-------|----------------|
| (none) | | | |

## Recommendations
1. **No action required for the gate.** Spec, code, and tests are aligned across
   all 14 FR + 7 SC.
2. **Doc staleness (minor, non-blocking, outside hash scope):** the component
   `CLAUDE.md` line "This crate must be added to the workspace … before building"
   is stale — `logger` is already a workspace member. Doc-only; not a spec/impl
   drift and outside the gate's `src/` + `specs/` hash scope.
