---
spec_sync_component: dispatcher-p2p
spec_sync_drift_status: drift
spec_sync_synced_at: 2026-09-02T21:32:13Z
spec_sync_git_commit: 2fc1cd3c
spec_sync_inputs_sha256: 51e1fa452b0d5eae2d3a74bab3ec6a99b9eedef2fc75723893be1e1ffc5a314a
spec_sync_hash_tool: scripts/spec-sync-hash.sh
---
# Spec ↔ Implementation Drift Report: dispatcher-p2p

**Spec analyzed**: `specs/001-gpudirect-cold-path/spec.md` (Status: Draft, Feature Branch: `p2p_component`, Last-Synced: 2026-08-20)
**Mode**: Read-only drift analysis (no build, no source modification).
**Cycle**: 2026-09-02 — re-analysis after the 2026-08-20 apply (SC-006 reworded, FR-018..FR-023 backfilled).

## Summary

| Metric | Count |
|--------|-------|
| Specs Analyzed | 1 |
| Requirements Checked | 29 (FR-001…023, SC-001…006) |
| Aligned | 28 |
| Drifted | 1 |
| Not Implemented | 0 |
| Unspecced Code Features | 1 |

The 2026-08-20 apply already reworded SC-006 to match the graceful-init/deferred-panic
behavior and backfilled FR-018..FR-023 for the previously unspecced
background/admin/async features; all of those are now **aligned** and were re-verified
against the current source this cycle. The **single remaining actionable drift is FR-017**:
the eviction drop-count is still never incremented on the live eviction paths, so
`eviction_dropped_count()` always returns 0. This is a code defect (spec is correct) and is
tracked as an ALIGN task — **not fixed by this pass** (sync does not modify `.rs`), hence
`drift_status: drift`.

## Detailed Findings — 001-gpudirect-cold-path

### Aligned ✓

- **FR-001** (SSD→GPU staging, bypass DRAM) — `ReadAsync` submitted directly into GPU BAR1 ring
  slots. `src/pipeline.rs` (p2p pipeline, ~750-805).
- **FR-002** (staging→client GPU D2D copy) — `cudaMemcpyAsync` DEVICE_TO_DEVICE in the p2p pipeline.
  `src/pipeline.rs`.
- **FR-003** (64-slot ring, cudaMalloc + GDRCopy BAR1 + spdk_mem_register, slot size from
  `max_transfer_size()`, 4 streams / min 2) — `P2P_RING_SLOTS=64` `src/p2p_ring.rs:19`;
  `NUM_STREAMS=4` `src/p2p_ring.rs:30`; ring init `src/p2p_ring.rs:53-118`; init log confirms
  slot count/size/streams `src/lib.rs:1201-1207`.
- **FR-004** (`ThreadPartition`, QD cap 16/thread, `MAX_QUEUES_PER_DRIVE`) —
  `MAX_QD_PER_THREAD=16` `src/p2p_ring.rs:25`; `MAX_QUEUES_PER_DRIVE` on `src/lib.rs`.
- **FR-005** (pipeline FIFO, round-robin across streams, sync per ring wrap, final sync) —
  round-robin `all_streams[completed % num_streams]` `src/pipeline.rs:795,986`; `sync_interval =
  ring_size.max(1)` `src/pipeline.rs:745,935`; boundary sync `src/pipeline.rs:812-818,1004`.
- **FR-006** (`batch_lookup` panics if ring uninitialized; single-key `lookup()` silent DRAM
  fallback) — panic via `.expect(...)` on the cold path `src/lib.rs:1753-1755`; single-key falls
  back to the DRAM/`promote_and_serve` path.
- **FR-007** (ring allocated once, immutable; production always P2P; single-key fallback for test) —
  ring written once at init `src/lib.rs:1207`; batch cold path requires `Some` `src/lib.rs:1752-1755`.
- **FR-008** (drop-in IDispatcher; `IRemoteLookup` fallback with `(key,size)`, dispatcher does the
  DRAM→GPU delivery) — `rl.batch_lookup(&remote_entries)` with `(CacheKey, u32)`
  `src/lib.rs:1906-1916`; local memory-tier→device delivery on success (remote path, ~1931-2030).
- **FR-009** (`DramBackfillWorker` after P2P serve; re-read SSD→DRAM, register MemoryTier;
  `backfill_delay_ms`) — worker `src/background.rs:236-301`; backfill job re-reads via
  `pipelined_ssd_to_dram_only` `src/lib.rs:1358`.
- **FR-010** (release staging on shutdown, no leaks) — `ring.destroy(&*gpu)` `src/lib.rs:1505,1509`;
  `P2pRing::destroy` frees streams + GPU allocs `src/p2p_ring.rs`.
- **FR-011** (read failures handled without corrupting ring / other ops) — per-chunk error return
  and per-job independent results in the p2p pipeline `src/pipeline.rs`.
- **FR-012** (performance via external tools, no built-in hooks) — no in-path instrumentation;
  hardware Criterion benches under `benches/`. Aligned.
- **FR-013** (`promote_to_memory_tier` uses `pipelined_ssd_to_dram_only`, one thread/drive, no P2P
  ring) — `src/lib.rs:2461,2578` calling `pipeline::pipelined_ssd_to_dram_only`
  (`src/pipeline.rs:1128`).
- **FR-014** (`backfill_delay_ms` default 10, 0 disables) — default in
  `../interfaces/src/idispatcher.rs`; `>0` gate + per-job sleep in the backfill path `src/lib.rs`.
- **FR-015** (`IGpuServices::set_device`/`device_of_ptr` exposed; per-device routing NOT yet wired
  into the p2p pipeline; mock-only) — no `.set_device(`/`.device_of_ptr(` call sites in the
  production cold path (only the test mock). Matches the spec's explicit "not yet wired" caveat.
- **FR-016** (`P2pColdReadPool` persistent per-(drive,queue) workers + pre-connected
  `ClientChannels`; inline fallback on creation failure; stopped before ring destroyed) — pool
  `src/cold_pool.rs`; created at init after ring `src/lib.rs:1248-1255`; non-fatal fallback log;
  inline fallback path `src/lib.rs:1825-1851`; pool stopped `src/lib.rs:1474` **before** ring
  destroy `src/lib.rs:1505,1509`.
- **FR-018** (`ParallelBackgroundWriter`, per-drive routing, in-flight/flush/shutdown, drop) —
  routing `device_index % num_drives` `src/background.rs:187-190`; `in_flight`/`flush`/`shutdown`
  `src/background.rs:193-212`; `Drop` calls shutdown `src/background.rs:215-219`; started
  `src/lib.rs:1287`.
- **FR-019** (`BackgroundEvictor`, SSD reclamation to low watermark, frees extents, emits `Removed`,
  honors shutdown/drop) — loop `src/background.rs:359-445`; aggregate utilization
  `src/background.rs:447-457`; batch of `batch_size` `src/background.rs:393`; `Removed` event
  `src/background.rs:414-419`; extent free `src/background.rs:424`; low-watermark stop
  `src/background.rs:432`; started `src/lib.rs:1400`; `Drop` `src/background.rs:482-488`.
- **FR-020** (`MemoryTierEvictor`, proactive DRAM→SSD demotion, pressure-scaled batch up to 8×,
  dry-run backoff, emits `Demoted`, honors shutdown/drop) — threshold gate
  `src/background.rs:567`; `multiplier = 1.0 + 7.0*pressure²` `src/background.rs:581`; widen-scan +
  backoff `src/background.rs:594,635-638`; demote via `try_evict_to_block`+`remove`
  `src/background.rs:603-609`; `Demoted` event `src/background.rs:611-616`; started
  `src/lib.rs:1428`; `Drop` `src/background.rs:648-654`.
- **FR-021** (`clear_memory_tier()` flush, demote-or-force-remove, returns count; requires init +
  bound receptacles) — `src/lib.rs:2610-2641` (`ensure_initialized`, dm/mt `.get()`, loop over
  `oldest_keys`, `try_evict_to_block` else force `remove`, returns `count`).
- **FR-022** (`lookup_async` returns `GpuStream`, warm-stream async H2D, sync fallback, caller
  synchronizes, pin released + LRU refreshed as part of the operation) — `src/lib.rs:2044-2143`
  (warm stream `memcpy_h2d_async` 2085-2102; sync fallback 2103-2131; `release_read` + `mt.touch`).
- **FR-023** (read-pin lifetime guard `PinnedKeys` across async copy completion) — guard type
  `src/pins.rs:26-57` (release-once-on-drop across all paths); used on the remote-lookup delivery
  path `src/lib.rs:1931,1949` with release **after** `stream_synchronize` `src/lib.rs:2026`; the
  local batch hot-path async copy synchronizes (`stream_synchronize` `src/lib.rs:1659`) **before**
  releasing its read pin (`release_read` `src/lib.rs:1697`). See the SC-006/observation note below
  regarding `lookup_async`.
- **SC-001** cold correctness single/multi client — classify + per-drive parallel cold path
  `src/lib.rs:1618-1888`.
- **SC-002** hot-path no regression — dedicated lock-free `warm_stream` for MemoryTier hits
  `src/lib.rs:1644-1699`.
- **SC-003** 4+ concurrent clients no corruption/deadlock — non-overlapping `ThreadPartition`
  ranges `src/p2p_ring.rs`, per-drive worker isolation `src/cold_pool.rs`.
- **SC-004** resources fully released on shutdown — pool stop then `ring.destroy`
  `src/lib.rs:1474,1505,1509`.
- **SC-005** throughput measurable P2P vs DRAM — external `certus-api-bench_v2.py` + hw benches.
- **SC-006** (init logs non-fatal diagnostic + continues; panic deferred to first cold
  `batch_lookup`; single-key `lookup()` falls back to DRAM) — **now aligned** after the 2026-08-20
  reword: init diagnostic `src/lib.rs:1209-1213`; deferred `.expect(...)` panic `src/lib.rs:1753-1755`.
  Matches FR-006/FR-007/User Story 2 AC-1.

### Drifted ⚠️

- **FR-017** — *moderate* (unchanged from prior cycle; code fix still pending).
  - Spec: "the event MUST be silently dropped **and counted**, and the running drop count MUST be
    readable and reset via `eviction_dropped_count()`."
  - Actual: the drop counter (`eviction_dropped.fetch_add`) lives **only** inside `emit_eviction`,
    which is `#[allow(dead_code)]` and **has no call sites** (`src/lib.rs:228-236`). Every live
    eviction publish site uses a bare `let _ = tx.try_send(...)` that discards the `Err` without
    incrementing the counter: `evict_for_space_*` inline path (`src/lib.rs:603-607,619-623,634-645`),
    `BackgroundEvictor::evictor_loop` (`src/background.rs:414-419`), and
    `MemoryTierEvictor::evictor_loop` (`src/background.rs:611-616`). Consequently
    `eviction_dropped_count()` (`src/lib.rs:224-226`) always returns 0, regardless of how many
    events are dropped. The channel / `try_send` / non-blocking / silent-drop parts of FR-017 are
    correct; only the drop-count guarantee is unmet.
  - Resolution: **ALIGN** (fix code). Task recorded in `.specify/sync/align-tasks.md`. Not applied
    by this pass (sync does not edit `.rs`).
  - Location: `src/lib.rs:224-236,602-648`, `src/background.rs:414-419,611-616`.

### Not Implemented ✗

None.

## Unspecced Code Features

| Feature | Location | Lines | Suggested Spec |
|---------|----------|-------|----------------|
| `cold_staging_slots` / `cold_staging_buf_bytes` config fields (unused by dispatcher-p2p; the 64-slot ring is governed by FR-003) | `components/interfaces/src/idispatcher.rs` | 84,87 (defaults 109,110) | HUMAN_DECISION — fields live on the shared `interfaces` config and are grep-unreferenced in `dispatcher-p2p/src/`. Cannot be edited by this sync (interfaces is out of scope). Wire in or remove from the config surface. |

The six previously-unspecced features (`ParallelBackgroundWriter`, `BackgroundEvictor`,
`MemoryTierEvictor`, `clear_memory_tier`, `lookup_async`, `PinnedKeys`) were backfilled as
FR-018..FR-023 in the 2026-08-20 cycle and are now specced/aligned.

## Inter-Spec Conflicts / Observations

- **FR-022 vs FR-023 — `lookup_async` pin lifetime** (*low, spec-internal*). FR-023 states the
  "pin must outlive the *completion* of an H2D copy, not its submission" and that the invariant
  "applies to both the local hot-path async copy and the remote-lookup delivery path." The two
  paths that batch-synchronize (the `batch_lookup` hot path, `src/lib.rs:1659→1697`, and the
  remote-lookup `PinnedKeys` path, `src/lib.rs:2026`) uphold this. However, `lookup_async`
  (FR-022) **releases the read pin at submission** (`src/lib.rs:2100`) before returning the stream
  for the caller to synchronize — exactly the "release at submission" pattern FR-023 warns against.
  FR-022's own wording sanctions this ("Read pins are released ... as part of the operation";
  "the caller is responsible for synchronizing the returned stream"), so the code matches FR-022
  and the two FRs can be read as governing different entry points. Flagged for a human to confirm
  whether `lookup_async`'s early release is intentional given the caller-synchronization contract,
  or a latent demote-during-copy race that should adopt the `PinnedKeys` guard. No code/spec change
  applied this cycle.

## Recommendations

1. **FR-017 (fix code)**: route all eviction publications through a single helper that increments
   `eviction_dropped` on `try_send` failure (channel full) *and* when no subscriber is registered —
   threading an `Arc<AtomicU64>` into `BackgroundEvictor::start`/`MemoryTierEvictor::start` — so
   `eviction_dropped_count()` reflects reality. Detailed task in `align-tasks.md`.
2. **cold_staging_* config (human)**: decide whether `cold_staging_slots`/`cold_staging_buf_bytes`
   should drive the ring (currently FR-003's `P2P_RING_SLOTS=64` is hard-coded) or be removed from
   the shared config; out of scope for this component's sync (lives in `interfaces`).
3. **FR-022/FR-023 pin lifetime (human)**: confirm `lookup_async`'s submission-time pin release is
   the intended contract or align it to the `PinnedKeys`-until-sync invariant.
4. **Plan staleness (minor)**: `plan.md` Source-Code layout omits `cold_pool.rs` and `pins.rs`, and
   its benches/tests names don't match the tree (actual benches: `dispatcher_hw_benchmark.rs`,
   `pipeline_hw_benchmark.rs`, `ssd_evictor_benchmark.rs`; no `tests/` dir). Doc-only.
