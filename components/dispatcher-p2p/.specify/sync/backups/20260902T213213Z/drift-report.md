# Spec ↔ Implementation Drift Report: dispatcher-p2p

**Spec analyzed**: `specs/001-gpudirect-cold-path/spec.md` (Status: Draft, Feature Branch: `p2p_component`)
**Mode**: Read-only drift analysis (no build, no source modification).

## Summary

| Metric | Count |
|--------|-------|
| Specs Analyzed | 1 |
| Requirements Checked | 23 (FR-001…017, SC-001…006) |
| Aligned | 21 |
| Drifted | 2 |
| Not Implemented | 0 |
| Unspecced Code Features | 7 |

Overall the implementation tracks the spec closely. The two drift items are (1) FR-017's
eviction drop-count is never actually incremented on the live eviction paths, and (2) SC-006's
"initialization panics" wording contradicts FR-006/User-Story-2, which the code follows (panic is
deferred to the first cold `batch_lookup`, not raised at init).

## Detailed Findings — 001-gpudirect-cold-path

### Aligned ✓

- **FR-001** (SSD→GPU staging, bypass DRAM) — `ReadAsync` submitted directly into GPU BAR1 ring
  slots. `src/pipeline.rs:750-759`, `src/pipeline.rs:798-804`.
- **FR-002** (staging→client GPU D2D copy) — `cudaMemcpyAsync` with `CUDA_MEMCPY_DEVICE_TO_DEVICE`.
  `src/pipeline.rs:798-804`.
- **FR-003** (64-slot ring, cudaMalloc + GDRCopy BAR1 + spdk_mem_register, slot size from
  `max_transfer_size()`, 4 streams / min 2) — `P2P_RING_SLOTS=64` `src/p2p_ring.rs:19`;
  `cudaMalloc` + `create_spdk_dma_buffer_from_gpu_bar` `src/p2p_ring.rs:58-80`; `NUM_STREAMS=4`
  with ≥2 fallback `src/p2p_ring.rs:30,84-118`; slot size = drive `max_transfer_size()`
  `src/lib.rs:1181-1186,1199`.
- **FR-004** (`ThreadPartition`, QD cap 16/thread, `MAX_QUEUES_PER_DRIVE`) —
  `MAX_QD_PER_THREAD=16` `src/p2p_ring.rs:25`; partition math `src/p2p_ring.rs:160-181`;
  `MAX_QUEUES_PER_DRIVE=1` `src/lib.rs:86`.
- **FR-005** (pipeline FIFO, round-robin across streams, sync per ring wrap, final sync) —
  round-robin `all_streams[completed % num_streams]` `src/pipeline.rs:795`; `sync_interval =
  ring_size` `src/pipeline.rs:745,815-830`; final sync `src/pipeline.rs:849-858`.
- **FR-006** (`batch_lookup` panics if ring uninitialized; single-key `lookup()` silent DRAM
  fallback) — panic via `.expect(...)` on the cold path `src/lib.rs:1752-1755`; single-key falls
  back to `pipeline_ring`/DRAM in `promote_and_serve` `src/lib.rs:459-523`.
- **FR-007** (ring allocated once, immutable; production always P2P; single-key fallback for test) —
  ring written once at init `src/lib.rs:1199-1214`; batch cold path requires `Some` `src/lib.rs:1752`.
- **FR-008** (drop-in IDispatcher; `IRemoteLookup` fallback with `(key,size)`, dispatcher does the
  DRAM→GPU delivery) — `rl.batch_lookup(&remote_entries)` with `(key, size)` `src/lib.rs:1906-1916`;
  memory-tier→device copy performed locally on success `src/lib.rs:1945-1983`.
- **FR-009** (`DramBackfillWorker` after P2P serve; re-read SSD→DRAM, register MemoryTier;
  `backfill_delay_ms`) — worker `src/background.rs:236-295`; enqueue after P2P
  `src/lib.rs:480-485,1883-1887`; backfill job re-reads and registers `src/lib.rs:1319-1373`.
- **FR-010** (release staging on shutdown, no leaks) — `ring.destroy(&*gpu)` and pipeline ring
  teardown `src/lib.rs:1503-1510`; `P2pRing::destroy` frees streams + GPU allocs
  `src/p2p_ring.rs:129-137`.
- **FR-011** (read failures handled without corrupting ring / other ops) — per-chunk error return
  `src/pipeline.rs:766-786`; per-job independent results in `pipelined_multi_object_p2p`
  `src/pipeline.rs:877-887`.
- **FR-012** (performance via external tools, no built-in hooks) — no in-path instrumentation;
  hardware Criterion benches exist under `benches/`. Aligned.
- **FR-013** (`promote_to_memory_tier` uses `pipelined_ssd_to_dram_only`, one thread/drive, no P2P
  ring) — `src/lib.rs:2457,2538-2583` (thread-scope per drive, `pipelined_ssd_to_dram_only`).
- **FR-014** (`backfill_delay_ms` default 10, 0 disables) — default `src/../interfaces/src/idispatcher.rs:61,102`;
  `>0` gate `src/lib.rs:1303`; per-job sleep `src/lib.rs:1326`.
- **FR-015** (`IGpuServices::set_device`/`device_of_ptr` exposed; per-device routing NOT yet wired
  into `pipelined_ssd_to_gpu_p2p`; mock-only) — trait methods
  `../interfaces/src/igpu_services.rs:555,577`; `_gpu` param unused in
  `pipelined_ssd_to_gpu_p2p` `src/pipeline.rs:705`; **no `.set_device(`/`.device_of_ptr(` call
  sites anywhere in `src/`** (only the test mock at `src/lib.rs:3350-3353`). Matches the spec's
  explicit "not yet wired" caveat.
- **FR-016** (`P2pColdReadPool` persistent per-(drive,queue) workers + pre-connected
  `ClientChannels`; inline fallback on creation failure; stopped before ring destroyed) — pool
  `src/cold_pool.rs:41-165`; created at init after ring `src/lib.rs:1242-1264`; non-fatal fallback
  log `src/lib.rs:1257-1261`; inline fallback path `src/lib.rs:1825-1851`; pool stopped
  `src/lib.rs:1474-1476` before ring destroy `src/lib.rs:1503`.
- **SC-001** cold correctness single/multi client — implemented via classify + per-drive parallel
  cold path `src/lib.rs:1618-1888`.
- **SC-002** hot-path no regression — dedicated lock-free `warm_stream` for MemoryTier hits
  `src/lib.rs:1642-1699,2085-2102`.
- **SC-003** 4+ concurrent clients no corruption/deadlock — non-overlapping `ThreadPartition`
  ranges `src/p2p_ring.rs:160-181`, per-drive worker isolation `src/cold_pool.rs`.
- **SC-004** resources fully released on shutdown — `src/lib.rs:1497-1511`.
- **SC-005** throughput measurable P2P vs DRAM — external `certus-api-bench_v2.py` + hw benches.

### Drifted ⚠️

- **FR-017** — *moderate*.
  - Spec: "the event MUST be silently dropped **and counted**, and the running drop count MUST be
    readable and reset via `eviction_dropped_count()`."
  - Actual: the drop counter (`eviction_dropped.fetch_add`) lives **only** inside
    `emit_eviction`, which is `#[allow(dead_code)]` and **has no call sites** (`src/lib.rs:228-236`).
    The live eviction paths — `evict_for_space_inner` (`src/lib.rs:602-607,618-646`),
    `BackgroundEvictor::evictor_loop` (`src/background.rs:414-419`), and
    `MemoryTierEvictor::evictor_loop` (`src/background.rs:611-616`) — all publish via
    `let _ = tx.try_send(...)`, discarding the `Err` **without** incrementing `eviction_dropped`.
    Consequently `eviction_dropped_count()` (`src/lib.rs:224-226`) always returns 0, regardless of
    how many events are dropped (full channel or no subscriber). The channel/`try_send`/non-blocking
    parts of FR-017 are correct; only the drop-count guarantee is unmet.
  - Location: `src/lib.rs:602-646`, `src/background.rs:414-419`, `src/background.rs:611-616`,
    `src/lib.rs:224-236`.
- **SC-006** — *minor* (spec-internal inconsistency; code follows FR-006).
  - Spec SC-006: "**Initialization** panics with a clear diagnostic when P2P ring allocation fails."
  - Actual: initialization does **not** panic; it logs a non-fatal diagnostic
    (`"P2P ring unavailable, cold reads use DRAM path"`, `src/lib.rs:1209-1213`). The panic is
    deferred to the first cold `batch_lookup` via `.expect("dispatcher-p2p requires P2P ring; use
    full.yaml profile for DRAM path")` (`src/lib.rs:1752-1755`). This matches FR-006 and User Story 2
    AC-1 ("On the first cold lookup attempt, it panics"), so the drift is between SC-006's wording and
    the rest of the spec, not a true implementation defect. Recommend rewording SC-006.
  - Location: `src/lib.rs:1209-1213`, `src/lib.rs:1752-1755`.

### Not Implemented ✗

None.

## Unspecced Code Features

| Feature | Location | Lines | Suggested Spec |
|---------|----------|-------|----------------|
| `MemoryTierEvictor` — proactive background DRAM→SSD demotion sweep (config `memory_tier_eviction_*`) | `src/background.rs` | 490-654 | Add FR for proactive memory-tier demotion; FR-017 only covers the event, not the sweep. |
| `BackgroundEvictor` — SSD extent eviction at utilization threshold (config `ssd_eviction_*`) | `src/background.rs` | 303-488 | Add FR for SSD capacity reclamation / watermarks. |
| `ParallelBackgroundWriter` — write-through persistence memory-tier→SSD, one thread/drive | `src/background.rs` | 154-219 | Spec the write/persistence path (spec is cold-read-only). |
| `clear_memory_tier()` — flush entire memory tier | `src/lib.rs` | 2606+ | Add FR or note as admin/maintenance op. |
| `lookup_async()` — returns `GpuStream` for caller-side pipelined hot-path sync | `src/lib.rs` | 2044-2110 | Document async single-key contract in interface spec. |
| `pins::PinnedKeys` — read-pin lifetime guard across async H2D copy (remote-lookup path) | `src/pins.rs` | 26-57 | Note pin-lifetime correctness invariant in data-model. |
| `cold_staging_slots` / `cold_staging_buf_bytes` config fields (appear unused by the P2P cold path) | `../interfaces/src/idispatcher.rs` | 82-87 | Confirm whether these apply to dispatcher-p2p or remove from its config surface. |

## Recommendations

1. **FR-017 (fix code)**: route all eviction publications through a single helper that increments
   `eviction_dropped` on `try_send` failure *and* when no subscriber is registered — i.e. actually
   use `emit_eviction` (or an `Arc`-shared counter passed into the evictor loops), so
   `eviction_dropped_count()` reflects reality. Currently the counter is permanently 0.
2. **SC-006 (fix spec)**: reword to "the component panics on the first cold `batch_lookup` when the
   P2P ring is unavailable" to align with FR-006 / User Story 2, or explicitly split init-time
   diagnostic vs. cold-lookup panic.
3. **Plan staleness (minor)**: `plan.md` Source-Code layout omits `cold_pool.rs` and `pins.rs`, and
   its benches/tests names (`benches/cold_path_benchmark.rs`, `tests/integration/`) don't match the
   tree (actual: `benches/dispatcher_hw_benchmark.rs`, `pipeline_hw_benchmark.rs`,
   `ssd_evictor_benchmark.rs`; no `tests/` dir). No stale `lib/` framework paths were found — the
   `components/component-framework`→`lib/` move is not mis-referenced in spec/plan/CLAUDE.md.
4. **Backfill FRs**: consider promoting the SSD/memory-tier background evictors and write-through
   worker into named requirements, since they are substantial always-on behaviors beyond the
   cold-read scope the spec describes.
