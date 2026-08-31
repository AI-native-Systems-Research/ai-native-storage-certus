---
spec_sync_component: dispatcher
spec_sync_drift_status: clean
spec_sync_synced_at: 2026-08-31T20:43:50Z
spec_sync_git_commit: 5bac8686
spec_sync_inputs_sha256: 1dd4ccc5158f01d0712e78d7aa778e716e67539f062d4714ca94508f0e01025f
spec_sync_hash_tool: scripts/spec-sync-hash.sh
---
# Spec Drift Report — dispatcher

Generated: pending
Project: dispatcher (spec: specs/001-dispatcher-cache-interface/spec.md)
Mode: Read-only drift analysis (no build, no source modification).
Branch: `sync-tmp`

## Summary

| Category | Count |
|----------|-------|
| Specs Analyzed | 1 |
| Requirements Checked (52 active FR + 15 active SC) | 67 |
| ✓ Aligned | 61 |
| ⚠️ Drifted | 5 |
| ✗ Not Implemented | 0 |
| 🆕 Unspecced Code | 1 (tier-event stats) |

Removed/superseded and therefore excluded from the active count: FR-020, FR-021,
FR-022 (prepare/commit/cancel_store direct-write workflow), FR-026, FR-027 (block
device / extent manager version selection), SC-008, and User Story 6.

This sweep re-analyzes the dispatcher after three post-2026-08-20 changes on this
branch: (1) `97e26738` removed gRPC and made the shm-queue the sole control
transport; (2) `995596e4` split the per-device warm CUDA stream into `warm_load`
(H2D) and `warm_store` (D2H) so both directions overlap on the PCIe bus; and
(3) `d8c26d58` moved the warm memory-tier↔GPU copies onto the batched
`IGpuServices::memcpy_batch_async` (`cuMemcpyBatchAsync`) API. A fourth change,
`4659626b`, added the `IDispatcher::tier_event_stats()` telemetry method, which
the dispatcher implements with a full `TierEventCounters` subsystem but which no
requirement covers.

Prior-report items that are **now aligned**: the User Story 11 queue-depth
intra-spec contradiction was fixed in the 2026-08-20 sync (`max_queue_depth = 128`
per thread throughout); the two `CLAUDE.md` path/`-v2` drifts are corrected in the
current tree (`../../lib/component-framework/crates/`, `block-device-spdk-nvme`);
and the dependency-injection / test surface is now specced as FR-057 / SC-016.

## Detailed Findings

### ⚠️ Drifted

- **FR-037 (warm stream: single → dual load/store; H2D copy API)** — severity: moderate.
  - Spec: "pre-allocate a **single warm CUDA stream** … used by `lookup_async` and
    `batch_lookup` for **`memcpy_h2d_async`** on raw memory-tier pointers … the per-device
    warm stream from `DEVICE_STREAMS`".
  - Actual: `DeviceStreams` now holds **two** warm streams per device — `warm_load`
    (memory-tier→GPU H2D) and `warm_store` (GPU→memory-tier D2H) — "Separate from
    `warm_load` so H2D and D2H DMA can overlap on the PCIe bus"
    (`src/lib.rs:335-345`); `device_streams_for` creates both plus the pipeline pair
    (`src/lib.rs:371-378`). The memory-tier→GPU H2D copy is issued via
    `IGpuServices::memcpy_batch_async` (`src/lib.rs:1127`), not `memcpy_h2d_async`
    (which now exists only in the test mock, `src/lib.rs:4193,4260`, and one test
    assertion, `:5244`). The `AtomicU64` `warm_stream` fallback is retained.
  - Location: `src/lib.rs:335-345,371-378,885,1127,2133,2740`.

- **FR-052 (per-device stream inventory)** — severity: minor.
  - Spec: "one warm stream plus one pipeline stream pair per device".
  - Actual: each device now has **two** warm streams (`warm_load`, `warm_store`) plus the
    pipeline pair (`DeviceStreams { warm_load, warm_store, pipe: [u64; 2] }`,
    `src/lib.rs:339-345`). The device-resolution / fallback-on-`-1` semantics FR-052
    describes are otherwise unchanged.
  - Location: `src/lib.rs:339-345`.

- **FR-006 / FR-039 step (2) / FR-056 `copy_gpu_to_memory_async` / Implementation Notes
    (warm-path copy API)** — severity: moderate.
  - Spec: warm-path H2D uses `IGpuServices::memcpy_h2d_async` (FR-006, FR-039(2),
    Implementation-Notes line 374); `copy_gpu_to_memory_async` "issues asynchronous
    GPU→host DMA … on the supplied CUDA stream" (FR-056, line 319); populate D2H uses
    `dma_copy_to_host`.
  - Actual: both warm directions now use the batched `memcpy_batch_async`
    (`cuMemcpyBatchAsync`) multi-region API. The memory-tier→GPU **scatter** (one DRAM
    slot → N client GPU regions) is a single `memcpy_batch_async` on the `warm_load`
    stream (`src/lib.rs:1116-1127`); the GPU→memory-tier **gather** in
    `copy_gpu_to_memory_async` (N client regions → one DRAM slot) is a single
    `memcpy_batch_async` on the `warm_store` stream (`src/lib.rs:2998-3009`). No
    production call site uses `memcpy_h2d_async` any more.
  - Location: `src/lib.rs:1116-1127,2984-3009`.

- **FR-040 (stale gRPC reference)** — severity: minor (doc/reference).
  - Spec: "The **gRPC handler** spawns this as a detached background task when
    `BatchTouchRequest.promote = true`."
  - Actual: gRPC was removed entirely and the shm-queue is now the sole control
    transport (`97e26738` — "Remove gRPC; make shm-queue the sole control transport").
    The `promote_to_memory_tier` API (`src/lib.rs:3060`) is unchanged; only the
    transport that invokes it changed. The sentence names a transport that no longer
    exists.
  - Location: spec FR-040; transport change `97e26738`.

- **FR-042 (stale gRPC reference)** — severity: minor (doc/reference).
  - Spec: "external consumers (e.g., **gRPC TakeEvents stream**) to observe cache
    evictions without polling."
  - Actual: same as FR-040 — the eviction-event channel
    (`create_eviction_channel` / `EvictionEvent`, `src/lib.rs:378,392-396`) is
    unchanged, but the example consumer named (gRPC TakeEvents) was removed in
    `97e26738`; the shm-queue transport is now the consumer.
  - Location: spec FR-042; transport change `97e26738`.

### ✗ Not Implemented

None. All active requirements have corresponding implementation.

## Unspecced Code

| Feature | Location | Lines | Suggested spec |
|---------|----------|-------|----------------|
| `tier_event_stats() -> TierEventStats` — cumulative tier-transition counters (promotions, demotions, cold serves, staged serves, remote fills, etc.) exposed on `IDispatcher`, backed by a lock-free `TierEventCounters` subsystem incremented on every tier transition. Commit `4659626b`. | `components/dispatcher/src/lib.rs` (impl) + `components/interfaces/src/idispatcher.rs` (trait) | lib.rs:111-160 (`TierEventCounters`, `snapshot`), 317 (`tier_counters` field), 3390 (trait impl); idispatcher.rs:189-210 (`TierEventStats`), 564 (`fn tier_event_stats`) | New **FR-058** (tier-event counters, always-on, unlike telemetry-gated `read_write_stats`) + **SC-017** (counters observable and monotonic across a populate/lookup/evict cycle). |
| Dependency-injection / test hooks `set_block_device_factory`, `set_extent_manager_factory`, `set_pipeline_metrics` | `components/dispatcher/src/lib.rs` | 360-372 | Already specced as **FR-057 / SC-016** (2026-08-20). No action. |

## Out-of-Scope Notes (not fixed by this sync)

- Two **source** comments still say "gRPC handler" (`src/lib.rs` ~2983, the
  null-stream comment near `copy_gpu_to_memory_async`). Source files are outside this
  sync's editable scope (`.specify/sync/**` and `specs/**` only); flagged for a
  follow-up source-comment cleanup with the same `97e26738` rationale.

## Recommendations

1. **FR-037 / FR-052 (backfill)**: update the warm-stream description to the shipped
   two-stream-per-device model (`warm_load` H2D + `warm_store` D2H, split for PCIe
   bidirectional overlap) and correct FR-052's per-device stream inventory.
2. **FR-006 / FR-039 / FR-056 / Impl-Notes (backfill)**: replace the
   `memcpy_h2d_async` / `dma_copy_to_host` warm-path references with the batched
   `memcpy_batch_async` (`cuMemcpyBatchAsync`) scatter/gather that ships today.
3. **FR-040 / FR-042 (backfill)**: replace the two stale gRPC references with the
   shm-queue control transport (`97e26738`).
4. **FR-058 + SC-017 (backfill-unspecced)**: document `tier_event_stats()` and its
   `TierEventCounters` subsystem.
