# Spec Sync Proposals
Generated: 2026-09-01
Project: dispatcher
Spec: 001-dispatcher-cache-interface
Source: `.specify/sync/drift-report.md`
Branch: `evolve-dispatcher`

Summary: 7 BACKFILL proposals (9 drifted requirements + 3 unspecced), all APPROVED interactively. No ALIGN, no HUMAN_DECISION. Editing scope: `specs/**` only (source untouched).

---

## Proposal 1 — FR-037 / FR-052 (warm stream field rename) [APPROVED]

- **Direction**: BACKFILL (code authoritative — spec names stale)
- **Requirement**: FR-037, FR-052
- **Commit**: `c410ac44`
- **Rationale**: `DeviceStreams` fields renamed from `warm_load`/`warm_store` to
  `warm`/`store` (`src/lib.rs:339-344`). The split persists (H2D and D2H still on
  separate streams for PCIe overlap), only the names changed.
- **Before**: "`warm_load` for memory-tier→GPU (H2D) copies and `warm_store` for
  GPU→memory-tier (D2H) copies".
- **After**: "`warm` for memory-tier→GPU (H2D) copies and `store` for GPU→memory-tier
  (D2H) copies". FR-052 inventory: "`warm` (H2D) + `store` (D2H) + pipe pair".

---

## Proposal 2 — FR-006 / FR-039(2) (warm H2D: batch→per-region) [APPROVED]

- **Direction**: BACKFILL (code authoritative)
- **Requirement**: FR-006, FR-039 step (2)
- **Commit**: `c410ac44`
- **Rationale**: `serve_memory_tier_to_gpu` now uses per-region `memcpy_h2d_async`
  calls on the device's `warm` stream (`src/lib.rs:1133-1144`) instead of a single
  `memcpy_batch_async`. This simplifies the hot path — the `cuMemcpyBatchAsync`
  kernel launch overhead amortizes only for many small regions; for 1-2 regions the
  per-call path is lower latency. The `memcpy_batch_async` mock impl was also
  removed from test code.
- **Before**: "SHOULD use `IGpuServices::memcpy_batch_async` (`cuMemcpyBatchAsync`)
  on the device's `warm_load` stream … scattering the DRAM slot to the client's GPU
  region(s) in a single batched call".
- **After**: "SHOULD use `IGpuServices::memcpy_h2d_async` for each destination region
  on the device's `warm` stream (FR-037), transferring contiguous DRAM slot bytes to
  each GPU region in sequence. Falls back to synchronous `dma_copy_to_device` when no
  CUDA streams are available."

---

## Proposal 3 — FR-056 / Assumptions (D2H: batch→per-region, stream changes) [APPROVED]

- **Direction**: BACKFILL (code authoritative)
- **Requirement**: FR-056 (`copy_gpu_to_memory_async`), Assumptions
- **Commit**: `c410ac44`
- **Rationale**: `copy_gpu_to_memory_async` now uses per-region `memcpy_d2h_async`
  (`src/lib.rs:3106-3121`) instead of `memcpy_batch_async`. The null-stream
  resolution to `warm_store` was removed — the function passes the caller's stream
  through directly. `populate_from_gpu` (the single-entry populate caller) now resolves
  the device's `warm` stream for D2H (`src/lib.rs:2922`), not the dedicated `store`
  stream.
- **Before**: "issues asynchronous GPU→host DMA … as a single batched
  `memcpy_batch_async` (`cuMemcpyBatchAsync`) gather on the supplied CUDA stream
  (or, when a null stream is supplied, on the source device's `warm_store` stream)".
- **After**: "issues per-region `memcpy_d2h_async` calls on the supplied CUDA stream,
  gathering each IPC region contiguously into the slot. The caller resolves the
  appropriate device stream before calling; no null-stream fallback is performed
  internally."
- **Assumptions**: update D2H line from `memcpy_batch_async` gather on `warm_store` to
  per-region `memcpy_d2h_async` on the caller-supplied stream; note that
  `populate_from_gpu` uses the device's `warm` stream.

---

## Proposal 4 — FR-039(4-5) / FR-044 / User Story 11 (queues per drive: 2→1) [APPROVED]

- **Direction**: BACKFILL (code authoritative)
- **Requirement**: FR-039 step (4-5), FR-044, User Story 11 acceptance scenario 3
- **Commit**: `c410ac44`
- **Rationale**: Both `COLD_POOL_QUEUES_PER_DRIVE` (FR-044, `src/lib.rs:1888`) and
  `MAX_QUEUES_PER_DRIVE` in `batch_lookup` (FR-039, `src/lib.rs:2279`) changed from
  2 to 1. With the coalesced per-object H2D pipeline, one queue per drive saturates
  the NVMe bandwidth — a second queue added no throughput but doubled channel pool
  pressure.
- **Before**: "spawns up to `MAX_QUEUES_PER_DRIVE` (default 2) threads per drive"
  / "entries are split into two groups of 5, each processed by a separate thread".
- **After**: "spawns up to `MAX_QUEUES_PER_DRIVE` (default 1) threads per drive. With
  the coalesced per-object H2D pipeline (`pipelined_multi_object_zero_copy`), one
  queue saturates NVMe device bandwidth."
- **User Story 11 SC-3**: "entries are processed by a single thread per drive" (remove
  the two-group-of-5 narrative).

---

## Proposal 5 — FR-019 / Key Entities / User Story 9 (coalesced per-object H2D) [APPROVED]

- **Direction**: BACKFILL (code authoritative)
- **Requirement**: FR-019, Key Entities (Pipelined Reader), User Story 9
- **Commit**: `ba1ac0a2` (pipeline coalesce), `c410ac44` (promote_and_serve switch)
- **Rationale**: `pipelined_multi_object_zero_copy` now tracks per-object segment
  completions and issues ONE coalesced `memcpy_h2d_async` per object after its last
  NVMe segment completes (`pipeline.rs:625-653`), instead of per-segment
  `dma_copy_to_device_async`. `PIPELINE_RING_SIZE` raised from 8 to 32 (now counts
  completed-object H2D syncs, not per-segment). `promote_and_serve` now calls the
  multi-object pipeline with a single `ColdReadJob` and `max_queue_depth=128`
  (`src/lib.rs:748-773`) instead of `pipelined_ssd_to_gpu_zero_copy` with
  `max_queue_depth=16`.
- **Before**: "As each NVMe read completes, the GPU H2D async DMA copy for that segment
  is issued immediately … A periodic stream sync every `PIPELINE_RING_SIZE` (8) GPU
  commands" / "The single-entry `promote_and_serve` path uses `max_queue_depth=16`."
- **After**: "The multi-object pipeline tracks per-object completion: NVMe segments
  complete and the next read is resubmitted immediately (preserving NVMe/GPU overlap),
  but H2D is deferred until all segments for an object are done, then ONE
  `memcpy_h2d_async` copies the entire contiguous DRAM slot to the GPU. A periodic
  stream sync every `PIPELINE_RING_SIZE` (32) completed-object H2D copies bounds the
  GPU queue depth." / "`promote_and_serve` uses `pipelined_multi_object_zero_copy`
  with a single `ColdReadJob` and `max_queue_depth=128`. The single-entry
  `pipelined_ssd_to_gpu_zero_copy` (per-segment H2D, `max_queue_depth=16`) is retained
  as the fallback for non-SPDK-registered memory."

---

## Proposal 6 — FR-059 + FR-001 (backfill `batch_populate`) [APPROVED]

- **Direction**: BACKFILL-UNSPECCED (code ships with no requirement)
- **Requirement**: new FR-059, update FR-001 method inventory
- **Commit**: `c410ac44`
- **Rationale**: `batch_populate(entries: &[(CacheKey, IpcHandle)]) -> Vec<Result<(),
  DispatcherError>>` ships on `IDispatcher` (`interfaces/src/idispatcher.rs:420-427`)
  and is implemented (`src/lib.rs:2940-3031`). It amortizes GPU stream synchronization:
  resolves the batch device, picks the dedicated `store` stream, interleaves
  `reserve_memory` + `copy_gpu_to_memory_async` for each key, issues ONE
  `stream_synchronize` for the batch, then registers all entries via
  `copy_gpu_to_memory_completed` and enqueues write-through.
- **Spec text**: New FR-059 + update FR-001 with `batch_populate` in the write/cache
  management list.

---

## Proposal 7 — FR-039 addendum (single-key cold inline bypass) [APPROVED]

- **Direction**: BACKFILL-UNSPECCED (perf optimization ships with no requirement)
- **Requirement**: addendum to FR-039
- **Commit**: `c410ac44`
- **Rationale**: When `batch_lookup` encounters exactly one cold entry with one
  destination region (`cold_entries.len() == 1 && regions.len() == 1`), it calls
  `promote_and_serve` inline (`src/lib.rs:2260-2276`) instead of dispatching through
  the cold pool. This eliminates two context switches (dispatcher → pool worker →
  dispatcher) that add scheduling latency to every single-key cold load. Measured:
  cuts bs=1 cold load p99 from ~4400µs to ~1880µs.
- **Spec text**: Add to FR-039 or as an addendum: "Single-entry cold batches with one
  destination region bypass the cold pool and call `promote_and_serve` inline to
  avoid thread-hop latency."
