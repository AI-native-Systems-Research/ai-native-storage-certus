# Spec Sync Apply Report: block-device-filesys

**Mode**: AUTO-BACKFILL
**Date**: 2026-07-22
**Source**: `.specify/sync/drift-report.{json,md}` (spec `001-block-device-filesys`)
**Scope**: Markdown only, under `specs/001-block-device-filesys/**` and `.specify/sync/**`. No source code was modified.

Backups of every edited existing Markdown file were written to `.specify/sync/backups/` before editing.

## Summary

| Resolution | Count |
|---|---|
| BACKFILL (spec/companion-doc text updated) | 5 |
| NEW_SPEC | 0 |
| SUPERSEDE | 0 |
| Companion docs fixed (beyond the FR text itself) | 3 |
| DEFERRED to align-tasks.md (code defect / needs code change) | 6 |

## Resolutions Applied

### BACKFILL — spec.md FR text updated to match correct, intentional, tested behavior

1. **FR-002** (logging levels): reworded to describe actual levels — lifecycle events (connect/disconnect) at debug, initialization at info, only genuinely exceptional conditions (ring-creation fallback, fsync-SQE-push failure) at warn, and io_uring queue-full conditions surfaced as an error `Completion` rather than additionally logged.
2. **FR-006** (block size default): reworded — `block_size`/`num_blocks` are required, explicit constructor arguments with no implicit default; `block_size` has an enforced minimum of 512 (power of 2), not a default of 512.
3. **FR-007** (O_DIRECT fallback logging): added a clarifying sentence — the fallback warning is printed via `eprintln!` from the `config` module (not `ILogger`) because that code runs before the actor/logger exist.
4. New **FR-019** (telemetry contract) added, documenting `TelemetryStats`' counters/semantics (op count, min/max/mean latency, bytes, throughput) as a backfilled requirement — **while explicitly flagging** that latency accounting is currently a known defect (always 0), pointing to the corresponding align-task rather than claiming it works.
5. New **FR-020** (per-client completion backlog) added, documenting the `ClientSession.pending` FIFO backlog / `flush_pending` non-blocking delivery mechanism that prevents cross-client head-of-line blocking.
6. New Edge Case bullet added describing the full-completion-channel/multi-client backlog behavior.
7. Key Entities section: added `ClientSession` and `TelemetryStats` entries.

None of these touched the Edge Cases bullet on io_uring backpressure or the Assumptions bullet on telemetry feature-gate parity — those map to real code defects and were left as the (correct) target spec text; see align-tasks.md.

### Companion docs fixed (docs-fixed)

1. `data-model.md`: `block_size` field description corrected (no default, enforced minimum); added missing `pending` field to `ClientSession` table; added missing `tag`/`start_ns`/`bytes` fields to `InflightOp` table (with a note on the `start_ns` defect).
2. `quickstart.md`: usage example corrected — it previously called `BlockDeviceFilesysComponent::new(/* fields */)` plus `pub(crate)`-only setters (`set_file_path`/`set_block_size`/`set_num_blocks`) which are not part of the public API; replaced with the real public constructor `create(file_path, block_size, num_blocks)`. Configuration table corrected to show block_size as a required parameter with an enforced minimum, not a default.
3. `contracts/iblock-device-contract.md`: `max_queue_depth()` entry corrected — it claimed the io_uring SQ size was "configurable, default 128"; it is actually a fixed constant (`DEFAULT_RING_DEPTH = 128`) with no runtime configuration.

## Deferred to align-tasks.md (NOT backfilled — code-side defects/gaps)

All items below are explicitly excluded from spec rewriting per the hard rules; the spec was left describing the intended/correct behavior and a task was filed to bring code into conformance (or to make an explicit, human-reviewed decision to descope).

1. **major** — `telemetry-latency`: telemetry always records 0ns latency (hard-coded in `record_op(0, ...)` calls; `InflightOp.start_ns` is dead/buggy) instead of real per-op timestamps.
2. **major** — `io-uring-backpressure`: no backpressure/retry on io_uring submission-queue-full; actor fails the op immediately instead of buffering and retrying as ring space frees up.
3. **moderate** — `FR-015-doc-examples`: `open_or_create_backing_file` has no doc example; `create()`'s example is fenced ` ```ignore ` so it never runs as a doctest, violating the project's runnable-doc-example convention.
4. **minor** — `SC-002-concurrency-throughput`: no test/benchmark asserts the "100 concurrent ops/sec, no corruption" criterion.
5. **minor** — `SC-003-sync-latency-bound`: no test/CI enforces the "<1ms sync 4KB latency" criterion; `benches/latency.rs` only measures, doesn't assert.
6. **minor** — `SC-005-benchmark-cv-bound`: no automated check enforces Criterion's "<15% coefficient of variation" criterion.

## Files Touched

- `components/block-device-filesys/specs/001-block-device-filesys/spec.md`
- `components/block-device-filesys/specs/001-block-device-filesys/data-model.md`
- `components/block-device-filesys/specs/001-block-device-filesys/quickstart.md`
- `components/block-device-filesys/specs/001-block-device-filesys/contracts/iblock-device-contract.md`
- `components/block-device-filesys/.specify/sync/align-tasks.md` (new)
- `components/block-device-filesys/.specify/sync/apply-report.md` (new, this file)
- `components/block-device-filesys/.specify/sync/backups/*` (pre-edit backups of the four Markdown files above)

No `.rs`, `Cargo.toml`, or `build.rs` files were modified.
