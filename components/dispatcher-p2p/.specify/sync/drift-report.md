# Spec Drift Report — dispatcher-p2p

Generated: 2026-07-22T22:35:42Z
Project: dispatcher-p2p

## Summary

| Category | Count |
|----------|-------|
| Specs Analyzed | 1 |
| Requirements Checked (FR+SC) | 21 |
| ✓ Aligned | 21 (100%) |
| ⚠️ Drifted | 2 (both in the `data-model.md` companion artifact, not `spec.md`) |
| ✗ Not Implemented | 0 |
| 🆕 Unspecced Code | 2 |

`spec.md` (FR-001..FR-015, SC-001..SC-006) is closely tracked by the implementation — no FR or SC text is contradicted by code, including the previously-flagged FR-015 (multi-GPU device routing), which now accurately documents its own gap. The drift found lives entirely in the **`data-model.md`** companion artifact, which has fallen behind two code changes. The more significant finding is **unspecced code**: the persistent cold-path worker pool (`cold_pool.rs`, added in commit `959fdd4`, "feat(dispatcher-p2p): add persistent cold-path worker pool") and an eviction-event notification channel are not mentioned anywhere in the spec package.

## Detailed Findings

### Spec: 001-gpudirect-cold-path — GPUDirect Storage Cold Path

#### Aligned ✓ (all FR/SC)

- **FR-001** (SSD→GPU staging, bypass DRAM) → `src/pipeline.rs:703` `pipelined_ssd_to_gpu_p2p`, `src/p2p_ring.rs`.
- **FR-002** (staging→client GPU D2D copy) → D2D copy calls inside `pipelined_ssd_to_gpu_p2p` / `pipelined_multi_object_p2p`, `src/pipeline.rs:703-950`.
- **FR-003** (64-slot ring, cudaMalloc + GDRCopy BAR1 + spdk_mem_register, MDTS-sized slots, 4 streams) → `src/p2p_ring.rs:19` (`P2P_RING_SLOTS = 64`), `:30` (`NUM_STREAMS = 4`), `:52-126` (`P2pRing::new`), `src/lib.rs:1183` (slot size from `max_transfer_size()`).
- **FR-004** (`ThreadPartition`, 16 slots/thread cap, `MAX_QUEUES_PER_DRIVE=1`) → `src/p2p_ring.rs:25,161-181`, `src/lib.rs:85`.
- **FR-005** (FIFO pipelining, round-robin over 4 streams, sync-per-wrap, final sync) → `src/pipeline.rs:688-742,877-950`.
- **FR-006** (batch_lookup panics if ring unavailable; init logs warning, doesn't fail; single-key lookup silently falls back to DRAM) → panic: `src/lib.rs:1738-1740` (`.expect(...)`); non-fatal init log: `src/lib.rs:1197-1210`; silent DRAM fallback: `src/lib.rs:457-522` (`promote_and_serve`).
- **FR-007** (ring immutable post-init; production always P2P; single-key path has DRAM fallback for test/staging) → `src/lib.rs:167-168` (`RwLock<Option<...>>` set once at init, never mutated after), `promote_and_serve` as above.
- **FR-008** (drop-in interface; `IRemoteLookup::batch_lookup` with `(key,size)`, dispatcher does DRAM→GPU delivery) → `src/lib.rs:1876-1957`.
- **FR-009** (`DramBackfillWorker`, throttled async promotion) → `src/background.rs:236-267` (`DramBackfillWorker`); enqueue calls at `src/lib.rs:479-484,1868-1872`.
- **FR-010** (release all staging resources on shutdown) → `src/lib.rs:1454-1510` (`shutdown()` calls `ring.destroy(&*gpu)`), `src/p2p_ring.rs:129-137` (`P2pRing::destroy`).
- **FR-011** (read failures don't corrupt ring / other in-flight ops) → `src/cold_pool.rs:125-164` (`worker_loop` returns per-job `Result`, ring untouched on error); `src/pipeline.rs` error paths propagate `Result` per-job.
- **FR-012** (perf measurement is external, not built-in) → no instrumentation in the hot production path; telemetry is feature-gated (`Cargo.toml`: `pipeline-telemetry`, `rw-telemetry`).
- **FR-013** (`promote_to_memory_tier`, no GPU involvement, one thread/drive) → `src/lib.rs:2342` (`fn promote_to_memory_tier`), uses `pipeline::pipelined_ssd_to_dram_only` (`src/pipeline.rs:1128`).
- **FR-014** (`backfill_delay_ms`, default 10ms, 0 disables backfill) → default at `components/interfaces/src/idispatcher.rs:102` (`backfill_delay_ms: 10`), gating logic at `src/lib.rs:1301-1325`.
- **FR-015** (`IGpuServices::set_device`/`device_of_ptr` present only in test mock; production `pipelined_ssd_to_gpu_p2p` does not yet route per-device) → confirmed: only implementations are the test mock at `src/lib.rs:3137-3141`; no call sites in `src/pipeline.rs`. The spec text (dated 2026-07-21) explicitly and accurately documents this as a known, not-yet-wired gap — scored aligned because the spec is honest about the current state rather than overclaiming.
- **SC-001, SC-003, SC-004** (correctness/concurrency/no-leak) → covered structurally by ring partitioning and `destroy()`/`shutdown()` cleanup, plus unit tests in `src/p2p_ring.rs:183-232` and `src/lib.rs` (e.g. `shutdown_without_initialize_succeeds`, `lookup_block_device_promote_without_hardware`).
- **SC-002** (hot path unaffected by P2P machinery) → dedicated `warm_stream` avoids taking the P2P/pipeline-ring locks on the hot path (`src/lib.rs:2002-2020`).
- **SC-005, SC-006** (measurable via external benchmark tool; init panics on ring failure with diagnostic) → `benches/` (`ssd_evictor_benchmark`, `pipeline_hw_benchmark`, `dispatcher_hw_benchmark`, the latter two gated by the `hardware-test` feature); panic path per FR-006.

Note: SC-002/003/005/006 are design-level guarantees backed by hardware-gated benchmarks/tests that cannot be executed in this (non-SPDK, non-GPU) analysis environment; classification is by code inspection, not a live run.

#### Drifted ⚠️ (companion artifact `data-model.md`, not `spec.md`)

- **requirement**: `data-model.md` — `P2pRing` entity, `streams` field
  **spec_text**: "`streams` | `[GpuStream; 2]` | Alternating CUDA streams for D2D copies"
  **actual**: 4 streams (`NUM_STREAMS = 4`), matching `spec.md` FR-003/FR-005 — `data-model.md` alone is stale.
  **location**: `components/dispatcher-p2p/specs/001-gpudirect-cold-path/data-model.md:12` vs `src/p2p_ring.rs:30,84-118`
  **severity**: minor (documentation-only; `spec.md` itself is correct)

- **requirement**: `data-model.md` — `PathSelection` entity (`OnceLock<PathSelection>` with `P2p(P2pRing) | DramFallback(PipelineRing)` variants)
  **spec_text**: "One-time decision stored for component lifetime... Storage: `OnceLock<PathSelection>`"
  **actual**: The component stores `p2p_ring: RwLock<Option<P2pRing>>` and `pipeline_ring: RwLock<Option<PipelineRing>>` as two independent fields (both can be populated simultaneously; the path is chosen per-call via `if let Some(ref p2p) = ...`, not via a single enum written once to an `OnceLock`).
  **location**: `components/dispatcher-p2p/specs/001-gpudirect-cold-path/data-model.md:58-67` vs `src/lib.rs:167-168,458-522`
  **severity**: moderate (the conceptual model in the data-model doc no longer matches the actual field layout)

#### Not Implemented ✗

None.

### Unspecced Code 🆕

| Feature | Location | Lines | Suggested Spec |
|---------|----------|-------|----------------|
| `P2pColdReadPool` — persistent cold-path worker pool. Pre-allocates one long-lived OS thread + pre-connected `ClientChannels` per (drive, queue-slot) at init, eliminating per-batch `connect_client()` + scoped-thread setup. Falls back to the previous inline per-batch path if pool creation fails. Commit `959fdd4` reports ~24% cold-path throughput improvement (11.5 → 14.2 GB/s). | `components/dispatcher-p2p/src/cold_pool.rs` (whole file); wired in `src/lib.rs:1247-1256` (init), `:1743-1842` (dispatch/fallback), `:1473` (shutdown) | 171 (cold_pool.rs) + ~100 (lib.rs integration) | Extend `001-gpudirect-cold-path` with a new FR near FR-004/FR-005 describing persistent per-drive workers as the primary execution path, with the inline connect-per-batch behavior as an explicit degraded fallback; update `data-model.md` to add a `P2pColdReadPool`/`WorkerHandle` entity |
| Eviction-event notification channel — `create_eviction_channel()`, `EvictionEvent`/`EvictionReason`, `eviction_dropped_count()`, emitted from `evict_for_space_inner` on every memory-tier eviction (demote/remove). Not referenced by any FR/SC or by `data-model.md`'s `LookupResult`/state-transition sections. | `src/lib.rs:50-60` (types), `:172-173` (fields), `:213-232` (`create_eviction_channel`, `eviction_dropped_count`, `emit_eviction`), `:546-650` (emission sites in `evict_for_space_inner`) | ~110 | New FR (or new spec, e.g. `002-eviction-telemetry`) describing observable eviction events, delivery semantics (bounded channel, drop-and-count on backpressure via `eviction_dropped_count`), and its consumer (plausibly the gRPC `TakeEvents` RPC added in sibling commit `4d5bd13`) |

## Inter-Spec Conflicts

None found — only one spec (`001-gpudirect-cold-path`) exists for this component.

## Recommendations

1. Add a new functional requirement (or amend FR-004/FR-005) documenting the persistent `P2pColdReadPool` as the primary cold-path execution model, and describe the inline per-batch fallback used when pool creation fails (`src/lib.rs:1256-1266`, `:1810-1836`) as an explicit degraded mode — this is user-visible via the ~24% throughput delta cited in the commit message.
2. Update `data-model.md`'s `P2pRing.streams` field from `[GpuStream; 2]` to the actual 4-stream ring (matching `spec.md` FR-003/FR-005), and add a `P2pColdReadPool` entity.
3. Rewrite `data-model.md`'s `PathSelection`/`OnceLock` description to match the actual two-field `RwLock<Option<P2pRing>>` / `RwLock<Option<PipelineRing>>` layout, or refactor the code to match the documented single-enum model — pick one direction and reconcile.
4. Write a spec (or extend the existing one) for the eviction-event channel (`EvictionEvent`, `eviction_dropped_count`) so its delivery/backpressure semantics are documented and testable, especially since it appears to feed an external gRPC `TakeEvents` consumer.
5. Consider adding a `README.md` for this component (currently absent) summarizing the P2P cold path, DRAM fallback, and worker-pool architecture, per the `component-update-docs` convention used elsewhere in the repo.
