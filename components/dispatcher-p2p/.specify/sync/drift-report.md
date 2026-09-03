---
spec_sync_component: dispatcher-p2p
spec_sync_drift_status: clean
spec_sync_synced_at: 2026-09-03T23:23:52Z
spec_sync_git_commit: d997f91e
spec_sync_inputs_sha256: 4cc9c570a1ad58484b94c9a65d2a028521ab34d18f62113424a628fb209827ca
spec_sync_hash_tool: scripts/spec-sync-hash.sh
---
# Spec ↔ Implementation Drift Report: dispatcher-p2p

**Generated**: 2026-09-03
**Spec analyzed**: `specs/001-gpudirect-cold-path/spec.md` (Status: Draft, Last-Synced 2026-09-03)
**Mode**: Read-only drift analysis, then **ALIGN** apply to code (spec authoritative for the FR-017 drop-count contract), + freshness stamp.

This sweep supersedes the earlier stale artifact (which read "Mode: Read-only,
no build", listed **2 Drifted** — FR-017 + SC-006 — and **7 Unspecced**). It
predated the 2026-08-20 Phase B spec update. Six of the seven earlier findings
are already resolved **in the spec**, and the last drift is resolved **in code**
this sweep:

- **SC-006 is no longer drifted** — the 2026-08-20 Phase B reworded SC-006 to the
  implemented behavior: init logs a **non-fatal** diagnostic and continues, and
  the failure is surfaced fatally on first use (the first cold `batch_lookup`
  panics; single-key `lookup()` falls back to DRAM). Verified against
  `src/lib.rs:1209-1213` (non-fatal init log) and `src/lib.rs:1752-1755`
  (deferred panic). SC-006 now matches FR-006/FR-007/User-Story-2 AC-1.
- **Five of the seven "unspecced" features are now specced** (2026-08-20 Phase B
  backfill): `ParallelBackgroundWriter` → **FR-018**; `BackgroundEvictor` (SSD
  reclamation) → **FR-019**; `MemoryTierEvictor` (DRAM→SSD demotion) → **FR-020**;
  `clear_memory_tier()` → **FR-021**; `lookup_async()` → **FR-022**;
  `pins::PinnedKeys` → **FR-023**. All also appear under **Key Entities**.
- **FR-017 drop-count is fixed in code this sweep** (see below).

## Summary

| Metric | Count |
|--------|-------|
| Specs Analyzed | 1 (`001-gpudirect-cold-path`) |
| Requirements Checked | 23 FR (FR-001…023) + 6 SC (SC-001…006) + 10 Key Entities |
| Aligned | 38 |
| Drifted (this sweep) | 1 → resolved by **ALIGN** (code fix) |
| Not Implemented | 0 |
| Unspecced | 1 (`cold_staging_*` interface fields — HUMAN_DECISION, non-gate-blocking) |

**Verification runs this sweep** (all green):
- `cargo build -p dispatcher-p2p` — clean
- `cargo clippy -p dispatcher-p2p --all-targets -- -D warnings` — clean for
  dispatcher-p2p's own sources (the extent-manager path-dependency has 13
  pre-existing `-D warnings` clippy lints — `manual_div_ceil`,
  `too_many_arguments` from `define_component!`, etc. — that are independent of
  this change; confirmed present with this sweep's edits stashed).
- `cargo test -p dispatcher-p2p -- --test-threads 1` — 71 passed; 0 failed,
  including the two new FR-017 tests
  (`publish_eviction_counts_only_undeliverable_emits`,
  `eviction_dropped_count_tracks_emit_path`).

## Detailed Findings — 001-gpudirect-cold-path

### Drifted ⚠️ → resolved by ALIGN (code fix)

- **FR-017 — eviction drop-count was never incremented on the live emit paths** —
  severity: moderate (code defect; spec authoritative — HARD RULE against
  backfilling a spec to match a bug).
  - Spec: FR-017 requires that when an eviction event cannot be delivered (channel
    full **or** no subscriber registered) "the event MUST be silently dropped
    **and counted**, and the running drop count MUST be readable and reset via
    `eviction_dropped_count()`."
  - Actual (before this sweep): the `eviction_dropped.fetch_add` lived **only**
    inside `emit_eviction`, which was `#[allow(dead_code)]` and had **zero call
    sites**. All four live emit paths published via a bare
    `let _ = tx.try_send(...)`, discarding the `Err` without incrementing the
    counter — so `eviction_dropped_count()` was permanently `0`:
    `evict_for_space_inner` (three sites), `BackgroundEvictor::evictor_loop`, and
    `MemoryTierEvictor::evictor_loop`.
  - Direction — **code authoritative (ALIGN)**: the FR-017 wording is the desired
    contract; the code had a defect. (Note: the sibling `dispatcher` component's
    FR-042 speaks only of the full-channel case, but dispatcher-p2p's own FR-017
    and `data-model.md` explicitly require counting the **no-subscriber** case
    too, so the fix counts both.)
  - Fix applied: introduced a shared free helper
    `publish_eviction(tx, dropped, key, reason)` (`src/lib.rs`) that increments
    the counter on **any** non-delivery (full channel or no subscriber) when
    `dropped` is `Some`, and is a pure no-op when `dropped` is `None`. All four
    live emit sites now route through it; the internal, deliberately
    non-emitting `evict_for_space` path passes `dropped = None` (so it neither
    publishes nor counts). `eviction_dropped` was widened from `AtomicU64` to
    `Arc<AtomicU64>` so the two background evictor threads
    (`BackgroundEvictor`/`MemoryTierEvictor`) share the same counter as the inline
    paths; the dead `emit_eviction` was deleted. Two unit tests were added
    (helper-level semantics and an end-to-end drive of the emit path asserting a
    non-zero count that resets on read).
  - Location (fixed): `src/lib.rs` (`publish_eviction` helper,
    `evict_for_space_inner`, `evict_for_space_emit`, `promote_to_memory_tier`
    scoped-thread path, the two `*Evictor::start` call sites),
    `src/background.rs` (`BackgroundEvictor` + `MemoryTierEvictor` `start`/
    `evictor_loop` signatures and publish sites).

### Aligned ✓ (verified this sweep)

- **FR-001** (SSD→GPU staging, bypass DRAM) — `ReadAsync` into GPU BAR1 ring slots.
  `src/pipeline.rs:750-759,798-804`.
- **FR-002** (staging→client GPU D2D copy) — `cudaMemcpyAsync` `DEVICE_TO_DEVICE`.
  `src/pipeline.rs:798-804`.
- **FR-003** (64-slot ring, cudaMalloc + GDRCopy BAR1 + spdk_mem_register, slot
  size from `max_transfer_size()`, 4 streams / min 2) — `P2P_RING_SLOTS=64`
  `src/p2p_ring.rs:19`; `NUM_STREAMS=4` w/ ≥2 fallback `src/p2p_ring.rs:30,84-118`.
- **FR-004** (`ThreadPartition`, QD cap 16/thread, `MAX_QUEUES_PER_DRIVE`) —
  `MAX_QD_PER_THREAD=16` `src/p2p_ring.rs:25`; `MAX_QUEUES_PER_DRIVE=1`
  `src/lib.rs:86`.
- **FR-005** (pipeline FIFO, round-robin streams, sync per ring wrap, final sync) —
  `src/pipeline.rs:745,795,815-830,849-858`.
- **FR-006** (`batch_lookup` panics if ring uninitialized; single-key silent DRAM
  fallback) — `.expect(...)` on cold path `src/lib.rs:1752-1755`; fallback
  `src/lib.rs:459-523`.
- **FR-007** (ring allocated once, immutable; batch cold path requires `Some`) —
  `src/lib.rs:1199-1214,1752`.
- **FR-008** (drop-in IDispatcher; `IRemoteLookup` fallback with `(key,size)`) —
  `src/lib.rs:1906-1916,1945-1983`.
- **FR-009** (`DramBackfillWorker` after P2P serve; `backfill_delay_ms`) —
  `src/background.rs:236-295`; enqueue `src/lib.rs:480-485,1883-1887`.
- **FR-010** (release staging on shutdown, no leaks) — `src/lib.rs:1503-1510`;
  `P2pRing::destroy` `src/p2p_ring.rs:129-137`.
- **FR-011** (read failures handled without corrupting ring / other ops) —
  `src/pipeline.rs:766-786,877-887`.
- **FR-012** (perf via external tools, no built-in hooks) — hardware Criterion
  benches under `benches/`; no in-path instrumentation.
- **FR-013** (`promote_to_memory_tier` uses `pipelined_ssd_to_dram_only`, one
  thread/drive, no P2P ring) — `src/lib.rs:2478,2559-2607`.
- **FR-014** (`backfill_delay_ms` default 10, 0 disables) —
  `../interfaces/src/idispatcher.rs:61,102`; gate `src/lib.rs:1303`.
- **FR-015** (`IGpuServices::set_device`/`device_of_ptr` exposed; per-device
  routing NOT yet wired; mock-only) — trait methods present; `_gpu` unused in
  `pipelined_ssd_to_gpu_p2p` `src/pipeline.rs:705`; no call sites outside the test
  mock. Matches the spec's explicit "not yet wired" caveat.
- **FR-016** (`P2pColdReadPool` persistent per-(drive,queue) workers; inline
  fallback on creation failure; stopped before ring destroyed) —
  `src/cold_pool.rs:41-165`; init `src/lib.rs:1242-1264`; stop
  `src/lib.rs:1474-1476` before destroy `src/lib.rs:1503`.
- **FR-017** — **now aligned** after this sweep's ALIGN fix (see above). The
  channel / `try_send` / non-blocking semantics were already correct; the
  drop-count guarantee is now met on all four emit paths.
- **FR-018** (`ParallelBackgroundWriter`, one writer thread/drive, in-flight
  accounting, `flush()`, draining `shutdown()`) — `src/background.rs:154-219`;
  routed by `device_index % num_drives`.
- **FR-019** (`BackgroundEvictor` SSD reclamation on `ssd_eviction_*` watermarks,
  emits `Removed`) — `src/background.rs:303-500` (publish now via
  `publish_eviction`).
- **FR-020** (`MemoryTierEvictor` DRAM→SSD demotion on `memory_tier_eviction_*`,
  pressure-scaled batches + dry-run backoff, emits `Demoted`) —
  `src/background.rs:509-654` (publish now via `publish_eviction`).
- **FR-021** (`clear_memory_tier()` flushes tier, requires init + bound
  receptacles) — `src/lib.rs` `clear_memory_tier`.
- **FR-022** (`lookup_async(key, ipc_handle)` returns `GpuStream`, warm-stream H2D
  with sync fallback, releases pins + refreshes LRU) — `src/lib.rs` `lookup_async`.
- **FR-023** (read pins held for full async-copy lifetime; `PinnedKeys` releases
  exactly once on drop across all exit paths) — `src/pins.rs:26-57`; used on both
  local hot-path async and remote-lookup delivery paths.
- **SC-001** cold correctness single/multi client — `src/lib.rs:1618-1888`.
- **SC-002** hot-path no regression — dedicated lock-free `warm_stream`
  `src/lib.rs:1642-1699,2085-2102`.
- **SC-003** 4+ concurrent clients no corruption/deadlock — non-overlapping
  `ThreadPartition` `src/p2p_ring.rs:160-181`; per-drive worker isolation.
- **SC-004** resources fully released on shutdown — `src/lib.rs:1497-1511`.
- **SC-005** throughput measurable P2P vs DRAM — external bench + hw benches.
- **SC-006** — **now aligned** (reworded 2026-08-20): non-fatal init diagnostic
  `src/lib.rs:1209-1213`; deferred fatal panic on first cold `batch_lookup`
  `src/lib.rs:1752-1755`; single-key DRAM fallback `src/lib.rs:459-523`.

### Key Entities — aligned ✓

Staging Ring, Ring Slot, Thread Partition, Dispatch Map, P2pColdReadPool,
EvictionEvent/EvictionReason, ParallelBackgroundWriter, BackgroundEvictor,
MemoryTierEvictor, and PinnedKeys all match their spec descriptions.

### Not Implemented ✗

None.

## Unspecced Features

| Feature | Location | Disposition |
|---------|----------|-------------|
| `cold_staging_slots` / `cold_staging_buf_bytes` config fields (unused by the P2P cold path) | `../interfaces/src/idispatcher.rs:84,87` (defaults 64 / 4 MiB `:109-110`) | **HUMAN_DECISION** — these live on the **shared** `IDispatcher` config interface, not on dispatcher-p2p's own surface, and are not referenced by `dispatcher-p2p/src`. Whether they apply to another `IDispatcher` implementor or should be removed is a cross-component interface decision, out of scope for a single-component ALIGN/BACKFILL. Non-gate-blocking for dispatcher-p2p. |

The other six items the stale report listed as "unspecced" are now specced
(FR-018…FR-023, see the header note).

## Recommendations

1. **FR-017 (done)**: resolved by the ALIGN code fix — the drop count is now
   incremented on every non-delivery across all four emit paths and read/reset via
   `eviction_dropped_count()`. Covered by two new unit tests.
2. **`cold_staging_*` (HUMAN_DECISION)**: raise on the interfaces owner — decide
   whether these `IDispatcher` config fields are consumed by any implementor or
   should be dropped. Not a dispatcher-p2p defect; does not block this component's
   gate.
3. **Plan staleness (minor, non-blocking)**: `plan.md`'s Source-Code layout omits
   `cold_pool.rs` and `pins.rs`, and its benches/tests names don't match the tree
   (actual: `benches/{dispatcher_hw,pipeline_hw,ssd_evictor}_benchmark.rs`; no
   `tests/` dir). Doc-only; outside the gate's src+spec hash scope.
