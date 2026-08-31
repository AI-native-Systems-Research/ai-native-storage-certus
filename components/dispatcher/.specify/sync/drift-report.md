# Spec Drift Report — dispatcher

Generated: 2026-08-31
Project: dispatcher (spec: specs/001-dispatcher-cache-interface/spec.md)

## Summary

| Category | Count |
|----------|-------|
| Specs Analyzed | 1 |
| Requirements Checked (52 active FR + 15 active SC) | 67 |
| ✓ Aligned | 65 |
| ⚠️ Drifted | 2 |
| ✗ Not Implemented | 0 |
| 🆕 Unspecced Code | 1 |

Removed/superseded and therefore excluded from the active count: FR-020, FR-021,
FR-022 (prepare/commit/cancel_store direct-write workflow), FR-026, FR-027 (block
device / extent manager version selection), SC-008, and User Story 6.

Since the 2026-08-20 (Phase B) sync, two things happened in the tree:
1. **gRPC was removed** (commit `97e26738`, 2026-08-18 — "Remove gRPC; make
   shm-queue the sole control transport"). The control transport is now the
   **shmq serve layer**. The 2026-08-20 "reconcile specs with code" pass
   (`a93b5620`) did not catch two residual `gRPC` references in the dispatcher
   spec (FR-040, FR-042).
2. A **tier-event counter subsystem** shipped (commits `4659626b` / `3231f85c` —
   "emit KV tier-event counts for the profiler"), adding a new `tier_event_stats()`
   method to the `IDispatcher` interface and a `TierEventCounters` type. No FR
   covers it.

The two CLAUDE.md documentation drifts recorded (but deferred as out-of-scope) by
the 2026-08-20 sync — the stale `../../component-framework/crates/` path and the
`-v2` crate names — have since been **corrected in `components/dispatcher/CLAUDE.md`**
(now `../../lib/component-framework/crates/`, and `block-device-spdk-nvme` /
`extent-manager` with no `-v2`). They are no longer drift.

Every other active functional-requirement data path, eviction path, lifecycle
step, config field (FR-033: all 18 `DispatcherConfig` fields present), and success
criterion remains present and matches code.

## Detailed Findings

### ⚠️ Drifted

- **FR-040 — stale gRPC control-transport reference** — severity: moderate.
  Spec says: *"The gRPC handler spawns this as a detached background task when
  `BatchTouchRequest.promote = true`."*
  Actual: gRPC has been removed (commit `97e26738`). The dispatcher's control-plane
  requests now arrive from the **shmq serve layer** (`src/lib.rs:5`, `:44`:
  *"Control-plane requests arrive from the shmq serve layer on blocking worker
  threads"*). `BatchTouchRequest` no longer appears anywhere in the dispatcher
  source. The `promote_to_memory_tier` method itself is unchanged and correct
  (`src/lib.rs:3060`); only the "who calls it" clause is stale.
  Direction: BACKFILL (the gRPC→shmq transport change is intentional and committed;
  code/architecture is authoritative).

- **FR-042 — stale gRPC consumer reference** — severity: minor.
  Spec says: *"This mechanism enables external consumers (e.g., gRPC TakeEvents
  stream) to observe cache evictions without polling."*
  Actual: the eviction channel is now drained by the **shmq serve layer** via
  `TakeEvents` (`src/lib.rs:442`: *"Returns the receiver that the shmq serve layer
  should drain via `TakeEvents`."*). The `create_eviction_channel` / `eviction_dropped_count`
  mechanism itself is unchanged and correct. Only the example-consumer clause is stale.
  Direction: BACKFILL.

### ✓ Aligned (representative evidence; all other active FR/SC verified present)

- FR-001 `IDispatcher` inventory — all listed lifecycle/read/write/lifecycle-primitive/
  introspection methods present in `components/interfaces/src/idispatcher.rs:238-556`.
  (Note: `tier_event_stats` at `:564` ships but is NOT yet listed in FR-001 — see Unspecced.)
- FR-002 `DispatcherError` (7 variants) — `idispatcher.rs`.
- FR-003/004/005 populate + write-through + slot retention — `lib.rs` populate path + `background.rs`.
- FR-006/007 lookup MemoryTier/BlockDevice + miss — `lib.rs:1982` → `lookup_async` (`lib.rs:2625`).
- FR-008 check; FR-009 remove; FR-018 non-blocking remove.
- FR-011 receptacles incl. `IRemoteLookup` + `poller_base_cpu`; FR-012 initialize validation.
- FR-014 shutdown drain + extent-manager `checkpoint()`; FR-015/016 N drives + FormatParams.
- FR-019 MDTS + `max_queue_depth`; single-entry `promote_and_serve` uses `16`, `batch_lookup` uses `128` (`lib.rs:2217`).
- FR-023 touch; FR-024 pin-safe `evict_for_space`/`evict_one_clean` + `MAX_SCAN`; `target_key` unused.
- FR-025 `format_on_init` recovery via `for_each_extent`/`recover_extent`.
- FR-029..033 SSD evictor + all 18 `DispatcherConfig` fields present (`idispatcher.rs` config struct),
  incl. `metadata_partition_size`, `extended_metadata_partition_size`, `backfill_delay_ms`, `max_eviction_attempts`.
- FR-034 `register_host_memory` + per-drive `ChannelPool`/`checkout`/`ChannelLease` (`lib.rs:181-239`);
  FR-035 `unregister_host_memory`.
- FR-036 `lookup_async` → `GpuStream`; FR-037 `warm_stream` + `DEVICE_STREAMS`/`device_streams_for`.
- FR-038 `clear_memory_tier`; FR-040 `promote_to_memory_tier` (`lib.rs:3060`); FR-041 `pipelined_ssd_to_dram_only`.
- FR-042 `create_eviction_channel(capacity)` + `eviction_dropped_count` + bounded `try_send` drop-count (`lib.rs:442`).
- FR-043 `PipelineMetrics` trait; FR-044 `ColdReadPool`; FR-045 remote-lookup merge (`remote_probe.rs`).
- FR-046..050 memory-tier evictor + quadratic pressure + `try_evict_to_block` + `EvictionEvent`.
- FR-051 `serve_concurrently_promoted`; FR-052 per-device streams; FR-053 `StagingPool`/`serve_cold_staged`.
- FR-054 drain-all/no-early-break; FR-055 `EXPECTED_PARTITIONS` guard.
- FR-056 reserve/copy/complete/release/pin/unpin + flush/stats.
- FR-057 DI/test setters (`set_block_device_factory`, `set_extent_manager_factory`, `set_pipeline_metrics`).
- SC-001..016 (excl. SC-008) exercised by tests in `lib.rs` and `tests/`.

### ✗ Not Implemented

None. All active requirements have corresponding implementation.

## Unspecced Code

| Feature | Location | Lines | Suggested spec |
|---------|----------|-------|----------------|
| KV-cache tier-event counters + `tier_event_stats()` `IDispatcher` method | `components/interfaces/src/idispatcher.rs` (trait method :564, `TierEventStats` struct :191); `components/dispatcher/src/lib.rs` (`TierEventCounters` :111-159, impl :3390, ~11 record sites in `lib.rs`/`background.rs`) | New FR (FR-058) + SC (SC-017), and add `tier_event_stats` to the FR-001 introspection inventory. Ships and is committed (profiler telemetry feature). Counters track promotions SSD→DRAM, lookups served→GPU, memory-tier evictions, and SSD-extent evictions; monotonic since process start; `snapshot()` reads without reset. |

## Inter-Spec Conflicts

None.

## Observations (out of sync scope — noted, not proposed)

- **Stale `gRPC handler` mentions in code comments** — `src/lib.rs:2983`, `:3016`
  ("e.g. gRPC handler", "pass null (gRPC handler)"). These are source code comments,
  outside this sync's editable scope (`.specify/sync/` and `specs/` only). Worth a
  follow-up code-comment pass to say "shmq serve layer / null-stream caller".
- **`components/dispatcher/verif/` reappeared** with Creusot proof artifacts
  (`evict_bound.coma`, `scan_widen.coma`, `segment_io.coma`, plus `target/creusot/`
  and `.why3find/`). It is **untracked/uncommitted local build state** (git status:
  `?? components/dispatcher/verif/`). The `IDispatcher` interface doc makes no
  verification claims (the 2026-08-07 sync removed the earlier Creusot overclaim),
  so there is no spec/interface claim to reconcile against it. Not actionable as
  spec drift.

## Recommendations

1. BACKFILL FR-040 and FR-042: replace the two stale `gRPC` clauses with the shmq
   serve layer (matching `src/lib.rs:5,44,442` and commit `97e26738`).
2. BACKFILL-UNSPECCED: add FR-058 + SC-017 for the tier-event counter subsystem and
   list `tier_event_stats` under FR-001's durability/introspection inventory.
3. (Out of scope) Follow-up code-comment pass to drop the two residual "gRPC handler"
   mentions in `src/lib.rs`.
