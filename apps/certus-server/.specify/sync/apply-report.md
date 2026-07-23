# Spec Apply Report

Generated: 2026-07-22
Project: certus-server
Based on: drift-report.md / drift-report.json (generated 2026-07-22T21:29:44Z)
Mode: AUTO-BACKFILL

## Summary

| Action | Count |
|--------|-------|
| Requirements Updated (Backfill) | 2 (FR-010, FR-020) |
| Requirements Added (Backfill) | 2 (FR-023 in spec 001; FR-014 in spec 002) |
| Requirements Left for Human Decision (Align Tasks) | 4 |
| Spec Files Modified | `specs/001-grpc-dispatcher-server/spec.md`, `specs/002-operational-config/spec.md` |
| Spec Files Unmodified (no drift) | `specs/003-otel-observability/spec.md` |
| Backups Created | 3 (see `.specify/sync/backups/`) |

## Applied Changes

### 1. FR-010 Updated — Python test client CLI/tests corrected to match actual code

**Direction**: BACKFILL (code → spec)
**Status**: APPLIED
**File**: `specs/001-grpc-dispatcher-server/spec.md`

**Before**: Documented CLI as `[--server ADDRESS:PORT] [--skip-tests] [--benchmark]` with a
9-case functional suite (`test_populate`, `test_populate_duplicate_key`,
`test_populate_already_exists`, `test_check`, `test_lookup`, `test_lookup_not_found`,
`test_remove`, `test_remove_not_found`, `test_touch`) and exit codes 0/1/2/3.

**After**: Documented CLI now matches `python-client/test_client.py`'s actual argparse
flags (`--server`, `--skip-large-batch`, `--bench`, `--bench-only`,
`--bench-object-size`, `--bench-num-objects`, `--bench-iterations`), the real
10-function test suite (`test_batch_populate`, `test_batch_check`, `test_batch_touch`,
`test_batch_lookup`, `test_batch_remove`, `test_check_after_remove`,
`test_duplicate_key_rejection`, `test_nonexistent_key_handling`,
`test_touch_nonexistent`, `test_large_batch`), an explicit note that there is no
AlreadyExists test case, and the real exit codes (`0`/`1` only).

**Rationale**: Client code and test names are authoritative per task direction; the
spec's Implementation Details section actively misled anyone trying to invoke the
client or map test names to acceptance scenarios.

---

### 2. FR-020 Updated — PendingStores keyed by client-supplied cache key, not a reservation ID

**Direction**: BACKFILL (code → spec)
**Status**: APPLIED
**File**: `specs/001-grpc-dispatcher-server/spec.md`

**Before**: "Pending stores are tracked per-reservation in a `PendingStores` map keyed
by a server-assigned reservation ID (`u64`)."

**After**: Corrected to state `PendingStores` is keyed by the client-supplied cache key
(the same `key` used by `Populate`/`Lookup`/etc.); `ReserveEntry` carries only `key` +
`size`, `CommitStore`/`AbortStore` take `keys`, and there is no reservation-ID
allocation anywhere in the protocol. A Clarifications entry (Session 2026-07-22) was
added explaining this is intentional: the cache key is already unique per entry, and
FR-015's duplicate-key pre-validation makes it safe to use directly as the reservation
handle, making a separate server-generated ID redundant.

**Judgment**: Treated as **intentional design**, not a lost feature — verified that
no reservation-ID field or allocation exists anywhere in `service.rs` or
`dispatcher.proto`, and that all four split-phase RPCs (`Reserve`/`CopyToStore`/
`CommitStore`/`AbortStore`) consistently use cache keys end-to-end. BACKFILL was
applied per task direction's guidance for this case.

---

### 3. FR-023 Added — `GetIoStats` RPC and `rw-telemetry` Cargo feature (spec 001)

**Direction**: BACKFILL (code → spec, new requirement)
**Status**: APPLIED
**File**: `specs/001-grpc-dispatcher-server/spec.md`

**Before**: No requirement covered the `GetIoStats` gRPC method or the `rw-telemetry`
Cargo feature that gates it.

**After**: New FR-023 documents `GetIoStats` (empty request → `IoStatsResponse` with
`read_ops`/`read_bytes`/`read_latency_ns_sum`/`write_ops`/`write_bytes`/
`write_latency_ns_sum`), gated by the optional `rw-telemetry` Cargo feature
(`rw-telemetry = ["dispatcher/rw-telemetry"]`), sourced from
`dispatcher.read_write_stats()`. Added `IoStatsResponse` to Key Entities and a
Clarifications note distinguishing this point-in-time diagnostics RPC from spec 003's
periodic OTel metrics export.

**Placement rationale**: Placed in spec 001 (gRPC API surface) rather than spec 003
(OTel observability) because `GetIoStats` is a plain gRPC RPC returning raw counters
on demand — it has no dependency on the `otel` feature and is not part of the
periodic OTLP export pipeline; it sits alongside spec 001's other diagnostic RPCs
(`TakeEvents`, FR-022).

---

### 4. FR-014 Added — `--memory-tier-eviction-threshold` CLI flag (spec 002)

**Direction**: BACKFILL (code → spec, new requirement)
**Status**: APPLIED
**File**: `specs/002-operational-config/spec.md`

**Before**: No requirement or CLI table row covered `--memory-tier-eviction-threshold`.

**After**: New FR-014 documents the flag (`f64`, default `0.0` = disabled; range
`(0.0, 1.0]` triggers background DRAM→SSD demotion at that memory-tier utilization
level), added a CLI Interface table row, and extended User Story 5 (Eviction Tuning)
with a third acceptance scenario covering the threshold-triggered demotion behavior.

---

## Deferred to Align Tasks (Human Decision Required)

See `apps/certus-server/.specify/sync/align-tasks.md` for full detail. Summary:

| # | Severity | Item | Reason Deferred |
|---|----------|------|------------------|
| 1 | Medium | Stale `README.md` (metadata-device/extent-manager architecture, incomplete RPC/CLI tables) | Not under `specs/**` — out of this pass's edit scope by hard rule |
| 2 | Low | FR-008: `EvictionPolicyLru`/`RemoteLookup` missing from spec 001 Component Stack | Outside this pass's explicit backfill scope (FR-010/FR-020/GetIoStats+rw-telemetry/eviction-threshold only) |
| 3 | Low | FR-011: `IpcHandle.offset` field undocumented | Outside this pass's explicit backfill scope |
| 4 | Low | Unused `ERROR_CODE_DUPLICATE_KEY` proto enum value | Ambiguous code-vs-spec resolution; any fix requires a `.proto`/`.rs` change this pass cannot make |

## Not Applicable

- **SUPERSEDE**: n/a — no spec was superseded by another in this pass.
- **NEW_SPEC**: Not warranted. Both unspecced items assigned an FR home (`GetIoStats`/
  `rw-telemetry` → spec 001 FR-023; `--memory-tier-eviction-threshold` → spec 002
  FR-014) fit naturally as additions to existing specs; neither represents a
  sufficiently distinct feature area to justify a new spec 004.

## Backups

Pre-edit copies saved to `apps/certus-server/.specify/sync/backups/`:
- `001-grpc-dispatcher-server.spec.md.20260722T162211.bak`
- `002-operational-config.spec.md.20260722T162211.bak`
- `apply-report.md.20260722T162211.bak` (previous report, from 2026-05-29 run)
