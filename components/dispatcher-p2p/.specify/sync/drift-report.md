# Spec Drift Report — dispatcher-p2p

Generated: 2026-08-07T15:31:21Z
Project: dispatcher-p2p (spec: specs/001-gpudirect-cold-path/spec.md)

## Summary

| Category | Count |
|----------|-------|
| Specs Analyzed | 1 |
| Functional Requirements (FR-001..017) | 17 |
| Success Criteria (SC-001..006) | 6 |
| ✓ Aligned | 23 |
| ⚠️ Drifted | 0 |
| ✗ Not Implemented | 0 |
| 🆕 Unspecced Code | 1 |

The GPUDirect cold-path implementation is fully aligned with `spec.md`. The P2P
staging ring, thread partitioning, panic/fallback policy, DRAM backfill worker,
persistent cold-read pool, and eviction-event channel are all present and match
the requirement text — including FR-015's explicit "not yet wired" note about
per-device routing, which the code confirms. The only issue is a shared-interface
doc block that references a nonexistent Creusot `verif/` proof tree.

## Detailed Findings

### Aligned

- **FR-001 / FR-002** — SSD→GPU-staging DMA then staging→client-GPU D2D copy: `pipelined_ssd_to_gpu_p2p` (`src/pipeline.rs:703`), `pipelined_multi_object_p2p` (`src/cold_pool.rs:153`).
- **FR-003** — fixed ring of 64 GPU staging buffers, slot size from drive `max_transfer_size`, 4 CUDA streams (min 2 fallback): `src/p2p_ring.rs` — `P2P_RING_SLOTS = 64` (`:19`), `NUM_STREAMS = 4` (`:30`), `cudaMalloc` per slot (`:58`), min-2 guard (`:108`), GDRCopy BAR1 mapping + `spdk_mem_register` documented/used (`:5,34`; register call `src/pipeline.rs:26`).
- **FR-004** — `ThreadPartition` non-overlapping slot ranges, effective QD capped at 16 per thread: `src/p2p_ring.rs:25 (MAX_QD_PER_THREAD=16), :161`; `MAX_QUEUES_PER_DRIVE` (`:9`).
- **FR-005** — FIFO completion ordering, D2D round-robin across streams, sync per ring wrap + final sync: `src/pipeline.rs` (`ring_size` sync interval, `streams()` round-robin).
- **FR-006 / FR-007** — `batch_lookup` panics if ring uninitialized ("dispatcher-p2p requires P2P ring; use full.yaml profile", `src/lib.rs:1754`); single-key `lookup()` silently falls back to DRAM via `lookup_async` (`src/lib.rs:1552-1563`); init logs non-fatal warning (`src/lib.rs:1211`); ring immutable after init (`RwLock<Option<P2pRing>>`, allocated once at `src/lib.rs:1199-1207`).
- **FR-008** — same `IDispatcher` interface (drop-in) + `IRemoteLookup` DRAM fallback with `(key,size)` pairs: receptacle `src/lib.rs:67`, mock `batch_lookup(&[(CacheKey,u32)])` `src/lib.rs:3524`.
- **FR-009 / FR-014** — `DramBackfillWorker` throttled by `backfill_delay_ms` (default 10, 0 disables): `src/background.rs:236`, start gated on `backfill_delay_ms > 0` `src/lib.rs:1300-1305`.
- **FR-010** — staging resources released on shutdown before ring destroy: `src/lib.rs:1497-1503` (`p2p_ring.write().take()`).
- **FR-011** — read-failure handling without ring corruption / drain semantics: pipeline drains completions (shared with dispatcher FR-054 pattern).
- **FR-012** — no built-in perf hooks; measurement external (`certus-api-bench_v2.py`) — consistent with `pipeline-telemetry` being an opt-in, off-by-default feature (`Cargo.toml`).
- **FR-013** — `promote_to_memory_tier` via `pipelined_ssd_to_dram_only` (no P2P ring): `src/lib.rs:2457,1358,2574`; `src/pipeline.rs:1128`.
- **FR-015** — `IGpuServices` receptacle exposes `set_device` / `device_of_ptr` (`src/lib.rs:3355,3358`), but per-device routing is NOT wired into `pipelined_ssd_to_gpu_p2p` — grep finds no `set_device`/`device_of_ptr` in `src/pipeline.rs` or `src/cold_pool.rs`. This exactly matches the spec's 2026-07-21 NOTE ("capability present in receptacle/mock; per-device routing not yet wired"). Aligned.
- **FR-016** — `P2pColdReadPool` persistent per-(drive,queue-slot) worker threads with pre-connected `ClientChannels`, non-fatal fallback to inline per-batch path on creation failure: `src/cold_pool.rs:41,61-63`.
- **FR-017** — `create_eviction_channel(capacity)` bounded single-subscriber + `evict_for_space_emit` (Demoted/Removed) with non-blocking `try_send`, drop-and-count via `eviction_dropped_count()`: `src/lib.rs:215-227,420`.
- **SC-001..006** — cold correctness / hot-path no-regression / 4+ concurrent / zero-leak shutdown / measurable throughput / init panic-on-ring-failure: covered by the FR implementations above and hardware benches (`benches/pipeline_hw_benchmark.rs`, `dispatcher_hw_benchmark.rs`).

### Not Implemented

None.

## Unspecced Code

| Item | Location | Notes |
|------|----------|-------|
| `set_block_device_factory` / `set_extent_manager_factory` DI hooks | `src/lib.rs:205,211` | Public test/injection hooks on the component struct; not described in spec (same pattern as the standard dispatcher). Minor. |

Note: the reserve/copy/complete/pin/unpin/flush_to_ssd/read_write_stats primitives
present in the impl are inherited from the shared `IDispatcher` interface and are
covered by FR-008 ("same interface as the standard dispatcher"), so they are not
counted as unspecced here (they are flagged in the standard dispatcher's report).

## Conflicts / Spec-Referencing-Nonexistent

- **Nonexistent Creusot proof tree (shared interface).** `components/interfaces/src/idispatcher.rs:185-198` claims Creusot "Verified Properties (see `components/dispatcher/verif/`)" (P1–P10, 24 VCs). No `verif/` directory exists under either `components/dispatcher/` or `components/dispatcher-p2p/`. The doc overstates the verification state for the interface this component implements.

## Recommendations

1. Leave FR-015 as-is (accurately describes current state) but track wiring per-device routing into `pipelined_ssd_to_gpu_p2p` as future work.
2. Optionally document the `set_*_factory` DI hooks or mark them `#[doc(hidden)]`/test-only.
3. Reconcile the shared-interface `verif/` proof references (restore proofs or soften the doc claim) — same finding as the standard dispatcher report.
