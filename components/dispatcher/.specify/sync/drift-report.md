---
spec_sync_component: dispatcher
spec_sync_drift_status: clean
spec_sync_synced_at: 2026-09-01T16:00:00Z
spec_sync_git_commit: c410ac44
spec_sync_inputs_sha256: 2fdfc2a12fae154112afeacc8e4d2fcccf57e0ba8f4983f781dfb6bae4be9c91
spec_sync_hash_tool: scripts/spec-sync-hash.sh
---
# Spec Drift Report — dispatcher

Generated: 2026-09-01
Project: dispatcher (spec: specs/001-dispatcher-cache-interface/spec.md)
Mode: Read-only drift analysis (no build, no source modification).
Branch: `evolve-dispatcher`

## Summary

| Category | Count |
|----------|-------|
| Specs Analyzed | 1 |
| Requirements Checked (53 active FR + 16 active SC) | 69 |
| ✓ Aligned | 57 |
| ⚠️ Drifted | 9 |
| ✗ Not Implemented | 0 |
| 🆕 Unspecced Code | 3 |

Removed/superseded and therefore excluded from the active count: FR-020, FR-021,
FR-022 (prepare/commit/cancel_store direct-write workflow), FR-026, FR-027 (block
device / extent manager version selection), SC-008, and User Story 6.

This sweep re-analyzes the dispatcher after two post-2026-08-31-sync commits on
this branch: (1) `ba1ac0a2` coalesced per-object H2D in the cold pipeline
(`pipelined_multi_object_zero_copy` now issues one `memcpy_h2d_async` per object
after all NVMe segments complete, replacing ~40 per-segment
`dma_copy_to_device_async` calls, and raised `PIPELINE_RING_SIZE` from 8 to 32);
and (2) `c410ac44` cut single-key cold load p99 by 57% via two changes — bypassing
the cold pool thread hop for single-entry cold batches (`batch_lookup` calls
`promote_and_serve` inline when `cold_entries.len()==1 && regions.len()==1`), and
switching `promote_and_serve` from `pipelined_ssd_to_gpu_zero_copy` to
`pipelined_multi_object_zero_copy` with a single `ColdReadJob`. These commits also
reverted the warm-path H2D API from `memcpy_batch_async` back to per-region
`memcpy_h2d_async`, reverted the D2H API from `memcpy_batch_async` to per-region
`memcpy_d2h_async`, renamed the `DeviceStreams` fields from `warm_load`/`warm_store`
to `warm`/`store`, reduced `COLD_POOL_QUEUES_PER_DRIVE` and `MAX_QUEUES_PER_DRIVE`
from 2 to 1, and added a `batch_populate` method on `IDispatcher`.

Prior-report items that were **resolved** by the Aug 31 sync: FR-037/FR-052 stream
split, FR-006/FR-039/FR-056 batch API, FR-040/FR-042 gRPC references, FR-058/SC-017
tier-event counters. Those are now re-drifted or newly drifted as described below.

## Detailed Findings

### ⚠️ Drifted

- **FR-037 (per-device warm stream field names)** — severity: minor.
  - Spec: "Each GPU device in the process-global `DEVICE_STREAMS` map is given
    **two** warm streams — `warm_load` for memory-tier→GPU (H2D) copies and
    `warm_store` for GPU→memory-tier (D2H) copies".
  - Actual: `DeviceStreams` still has two distinct warm/store streams per device, but
    the fields are now named `warm` (H2D) and `store` (D2H) — not `warm_load` and
    `warm_store` (`src/lib.rs:339-344`). `device_streams_for` creates both plus the
    pipeline pair (`src/lib.rs:370-387`).
  - Also: the doc comment now says "keep one warm stream and one pair of pipeline
    streams per device" (`src/lib.rs:333`) which is misleading — there are still two
    streams (`warm` + `store`) plus the pipe pair, i.e. four streams per device.
  - Location: `src/lib.rs:333,338-347`.

- **FR-006 (warm H2D copy API)** — severity: moderate.
  - Spec: "the MemoryTier path SHOULD use `IGpuServices::memcpy_batch_async`
    (`cuMemcpyBatchAsync`) on the device's `warm_load` stream (FR-037) with raw
    memory-tier pointers, scattering the DRAM slot to the client's GPU region(s)
    in a single batched call".
  - Actual: `serve_memory_tier_to_gpu` now issues **per-region `memcpy_h2d_async`**
    calls in a loop on the device's `warm` stream (`src/lib.rs:1133-1144`), not a
    single batched `memcpy_batch_async`. The batch API is no longer called anywhere
    in production code. The mock's `memcpy_batch_async` implementation was also
    removed (`src/lib.rs:4350` area — method deleted from mock).
  - Location: `src/lib.rs:1124-1145`.

- **FR-039 step (2) (batch_lookup warm H2D)** — severity: moderate.
  - Spec: "for MemoryTier hits, issues the H2D copies as batched `memcpy_batch_async`
    calls on the device's `warm_load` stream".
  - Actual: `batch_lookup` classifies hot entries and calls `serve_memory_tier_to_gpu`
    (or `serve_memory_tier_to_gpu_multi_region`), which uses per-region
    `memcpy_h2d_async` on the `warm` stream, not `memcpy_batch_async`.
  - Location: `src/lib.rs:2147` (warm_raw resolved), `src/lib.rs:1133-1144`
    (serve_memory_tier_to_gpu impl).

- **FR-039 step (4-5) / FR-044 (cold pool queues per drive)** — severity: minor.
  - Spec: "spawns up to `MAX_QUEUES_PER_DRIVE` (default 2) threads per drive"
    (FR-039 step 4); "pre-allocates per-drive, per-queue resources" (FR-044).
  - Actual: `COLD_POOL_QUEUES_PER_DRIVE` is now 1 (`src/lib.rs:1888`) and
    `MAX_QUEUES_PER_DRIVE` in `batch_lookup` is now 1 (`src/lib.rs:2279`).
    The cold pool still exists but has one queue per drive instead of two.
  - User Story 11 acceptance scenario 3 says "entries are split into two groups
    of 5, each processed by a separate thread" — now one group per drive.
  - Location: `src/lib.rs:1888,2279`.

- **FR-056 `copy_gpu_to_memory_async` (D2H copy API)** — severity: moderate.
  - Spec: "issues asynchronous GPU→host DMA of the given IPC regions into the
    reserved slot as a single batched `memcpy_batch_async` (`cuMemcpyBatchAsync`)
    gather on the supplied CUDA stream (or, when a null stream is supplied, on the
    source device's `warm_store` stream, FR-037)".
  - Actual: `copy_gpu_to_memory_async` now issues **per-region `memcpy_d2h_async`**
    calls in a loop (`src/lib.rs:3106-3121`), not `memcpy_batch_async`. The
    null-stream resolution to `warm_store` was removed — the function now passes the
    caller's stream through unchanged. Callers (`populate_from_gpu`) now resolve the
    `warm` (H2D) stream, not the `store` (D2H) stream.
  - Location: `src/lib.rs:3100-3123` (impl), `src/lib.rs:2922` (caller resolves `warm`).

- **FR-019 / Key Entities (pipelined reader H2D strategy)** — severity: moderate.
  - Spec: "As each NVMe read completes, the GPU H2D async DMA copy for that segment
    is issued immediately and the next NVMe read is submitted" / "A periodic stream
    sync every `PIPELINE_RING_SIZE` (8) GPU commands".
  - Actual: `pipelined_multi_object_zero_copy` now uses **coalesced per-object H2D**:
    NVMe segments complete and the next read is resubmitted, but the H2D copy is
    deferred until ALL segments for an object are complete, at which point ONE
    `memcpy_h2d_async` copies the entire contiguous DRAM slot to the GPU
    (`pipeline.rs:625-653`). The per-segment `dma_copy_to_device_async` calls are
    removed. `PIPELINE_RING_SIZE` is now 32 (per-object-H2D sync frequency, not
    per-segment). The single-entry `pipelined_ssd_to_gpu_zero_copy` retains
    per-segment H2D.
  - Location: `pipeline.rs:55 (PIPELINE_RING_SIZE=32), 559-690 (coalesced H2D loop)`.

- **FR-052 (per-device stream inventory field names)** — severity: minor.
  - Spec: "two warm streams (`warm_load` for H2D and `warm_store` for D2H, FR-037)
    plus one pipeline stream pair per device".
  - Actual: fields are `warm` (H2D) and `store` (D2H), not `warm_load`/`warm_store`.
    The inventory count (2 warm + 2 pipeline = 4 streams) is unchanged.
  - Location: `src/lib.rs:339-346`.

- **Assumptions (D2H API and stream references)** — severity: minor.
  - Spec: "a batched `memcpy_batch_async` (`cuMemcpyBatchAsync`) gather on the
    `warm_store` stream for the reserve/`copy_gpu_to_memory_async` path".
  - Actual: per-region `memcpy_d2h_async` on the caller's passed stream. The
    `populate_from_gpu` caller resolves the `warm` stream, not `warm_store`/`store`.
  - Also: "The `warm_load` and `warm_store` streams are distinct per device" — now
    `warm` and `store`.
  - Location: Assumptions section; `src/lib.rs:2922,3106-3121`.

- **Implementation Notes (User Story 9 / `promote_and_serve`)** — severity: minor.
  - Spec (User Story 9): "uses the sliding-window zero-copy pipeline
    (`pipeline::pipelined_ssd_to_gpu_zero_copy`)".
  - Actual: `promote_and_serve` now calls `pipelined_multi_object_zero_copy` with
    a single `ColdReadJob` and `max_queue_depth=128` (`src/lib.rs:748-773`), not
    `pipelined_ssd_to_gpu_zero_copy` with `max_queue_depth=16`. The multi-object
    pipeline uses coalesced per-object H2D instead of per-segment H2D.
  - Location: `src/lib.rs:748-773`.

### ✗ Not Implemented

None. All active requirements have corresponding implementation.

## Unspecced Code

| Feature | Location | Lines | Suggested spec |
|---------|----------|-------|----------------|
| `batch_populate(entries: &[(CacheKey, IpcHandle)]) -> Vec<Result<(), DispatcherError>>` — batch-populate multiple cache entries from GPU memory, amortizing stream synchronization: all D2H copies submitted async on a dedicated `store` stream, one `stream_synchronize` for the batch, then all entries registered in dispatch-map and enqueued for write-through. Each entry goes through `reserve_memory` → `copy_gpu_to_memory_async` → single sync → `copy_gpu_to_memory_completed`. | `components/interfaces/src/idispatcher.rs:420-427` (trait), `components/dispatcher/src/lib.rs:2940-3031` (impl) | ~90 | New **FR-059** + update FR-001 method inventory. |
| Single-key cold inline bypass in `batch_lookup`: when `cold_entries.len() == 1 && cold_entries[0].regions.len() == 1`, calls `promote_and_serve` inline instead of dispatching through the cold pool, eliminating two context switches (dispatcher → pool worker → dispatcher). | `components/dispatcher/src/lib.rs:2260-2276` | ~17 | New **FR-060** or addendum to FR-039 describing the single-entry fast path. |
| `promote_and_serve` now uses `pipelined_multi_object_zero_copy` with `max_queue_depth=128` for all invocations (including single-entry), not `pipelined_ssd_to_gpu_zero_copy` with `max_queue_depth=16`. The single-entry and batch cold paths now share the same pipeline. | `components/dispatcher/src/lib.rs:748-773` | ~25 | Update FR-019 / User Story 9 to reflect the unified pipeline and queue depth. |

## Out-of-Scope Notes (not fixed by this sync)

- Two **source** comments still say "gRPC handler" (`src/lib.rs` near
  `copy_gpu_to_memory_async` area). Source files are outside this sync's editable
  scope (`.specify/sync/**` and `specs/**` only); flagged for a follow-up source
  comment cleanup.
- The `DeviceStreams` doc comment at `src/lib.rs:333` says "one warm stream" but
  there are two (`warm` + `store`); a source comment fix is outside sync scope.

## Recommendations

1. **FR-037 / FR-052 (backfill)**: update field names from `warm_load`/`warm_store`
   to `warm`/`store` throughout the spec.
2. **FR-006 / FR-039 step (2) (backfill)**: replace `memcpy_batch_async` scatter with
   per-region `memcpy_h2d_async` on the device's `warm` stream.
3. **FR-056 / Assumptions (backfill)**: replace `memcpy_batch_async` gather on
   `warm_store` with per-region `memcpy_d2h_async` on the caller's passed stream;
   remove null-stream → `warm_store` resolution; note `populate_from_gpu` now uses
   `warm` not `store`.
4. **FR-039 steps (4-5) / FR-044 / User Story 11 (backfill)**: update
   `MAX_QUEUES_PER_DRIVE` default from 2 to 1.
5. **FR-019 / Key Entities / User Story 9 (backfill)**: describe coalesced per-object
   H2D in `pipelined_multi_object_zero_copy`; update `PIPELINE_RING_SIZE` from 8 to 32;
   note `promote_and_serve` now uses the multi-object pipeline with `max_queue_depth=128`.
6. **FR-059 (backfill-unspecced)**: document `batch_populate`.
7. **FR-060 or FR-039 addendum (backfill-unspecced)**: document the single-key cold
   inline bypass.
