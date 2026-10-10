---
spec_sync_component: dispatcher-p2p
spec_sync_drift_status: drift
spec_sync_synced_at: 2026-10-10T01:13:03Z
spec_sync_git_commit: 2775224b
spec_sync_inputs_sha256: 5df2820b0a2274b75f1398831bc9c28e1ec8cfa67a7c1adee07a31a1d184e6d8
spec_sync_hash_tool: scripts/spec-sync-hash.sh
---
> **Sync 2026-10-10 (same branch; spec-only).** FR-009 downgraded from MUST to MAY (backfill
> runs only when `backfill_delay_ms > 0`, verified at `src/lib.rs:1425`); FR-014 records that
> `certus-server-yaml` now defaults it to 0 (commit 2775224b) with the measurement behind it;
> FR-032 (frequency-gated promotion) and FR-033 (GPU→SSD write-around when the tier is full)
> added as explicitly FUTURE, not-implemented requirements, so they are not drift. Approved by
> the user. **Still `drift`:** D4 (clean-eviction invariant), D5 (store backpressure) and D7
> (FR-027/FR-028 anchors) remain open from the 2026-10-09 sync.

> **Sync 2026-10-09 (branch `fix/dispatcher-p2p-clean-eviction`).** Delta analysis on a certified
> baseline: the previous clean stamp was re-verified to equal the spec-sync hash of
> `git archive 3c001478`, so the only new input is this branch's `src/lib.rs` (the #220
> clean-eviction fix). Every FR/SC and cross-spec contract touching eviction, backpressure,
> `reserve_memory`, `tier_event_stats` or the FR-033/FR-034 counters was located by grep over
> `specs/**` and `dispatcher` spec 002, then re-checked against the changed code.
>
> - **D1 FR-017 — DRIFTED (moderate) → BACKFILL, applied.** Foreground eviction no longer has a
>   "remove" outcome; `Removed` now comes only from the SSD evictor. `src/lib.rs:606-655`.
> - **D2 FR-025a — DRIFTED (moderate) → BACKFILL, applied.** "No tier-movement counters" was
>   false: `evictions_from_memory` and `store_backpressure_events` are reported.
> - **D3 FR-029 — DRIFTED (major) → BACKFILL, applied.** Rewritten: the FR-033/FR-034 counters are
>   reported for the foreground paths; the declared gap is narrowed to the background
>   `MemoryTierEvictor` and the fields listed there. `src/lib.rs:83-117, 606-655, 2919-2950`.
> - **D6 `eviction_scans_exhausted` semantics vs dispatcher FR-034 — DRIFTED (minor) → ALIGN,
>   applied in code.** Was counted once per giving-up eviction; now once per scan that frees
>   nothing, as FR-034 and `dispatcher` do. Two test assertions updated; 77/77 pass.
> - **C1 conflict with dispatcher spec 002 FR-035 — resolved** by narrowing FR-035 (dispatcher
>   re-stamped, doc-only).
> - **D4 clean-eviction invariant (#220) — UNSPECCED (major), NOT APPLIED** (not approved this
>   sync). `src/lib.rs:606-743`.
> - **D5 store backpressure in `reserve_memory` — UNSPECCED (moderate), NOT APPLIED** (not
>   approved). `src/lib.rs:721-743, 2442-2530`.
> - **D7 stale anchors in FR-027 / FR-028 — DRIFTED (minor), NOT APPLIED** (not approved): now
>   `src/lib.rs:320-371, 975-984` and `src/lib.rs:2868`.
>
> **Stamped `drift`:** D4, D5 and D7 remain open by decision, not by oversight. Approving the
> proposed FR-030 / FR-031 and the anchor refresh would clear it.

> **Sync 2026-10-05 (branch `fix/store-declines-root-cause`).** Delta analysis on a certified baseline (see the interfaces report).
>
> - **Spec not updated while code moved — DRIFTED (moderate), two requirements' worth.** This component's `src/lib.rs` gained 37 lines and its own spec was untouched, which `dispatcher` spec 002 **FR-030** names as a MUST for exactly this component and spec. Two distinct gaps: a full `IDispatcher::schedule_write_through` implementation (a real `WriteJob` enqueue, not a no-op), and a declaration that the FR-033/FR-034 eviction counters are unreported here. FR-035 requires that second one be declared "in both the component's code and this spec" — the code comment existed, and the dispatcher's spec carried the note, but **this** component's spec said nothing. **Resolution**: new **FR-028** (the real enqueue, same `drive_index` placement hash as the FR-018 store path, non-blocking, best-effort, with why a no-op would be wrong under the `full-p2p` profile specifically) and new **FR-029** (the unmeasured-counter declaration, and why it is a declared gap rather than a defect — contrasted against FR-025a's self-contradictory zeroed route counters).
> - Both follow the precedent FR-025a and FR-027 set in this file for drift found by a sweep.
>
> No actionable drift remains for this component.
> **Sync 2026-10-05 (branch `fix/poller-cpu-placement-logging`).** Delta analysis on a certified baseline: this component's existing clean stamp was re-verified to equal the spec-sync hash of `git archive origin/unstable`, so the only new inputs are this branch's src/specs/interfaces changes. Every FR/SC touching CPU placement, NUMA pinning, threads or SPDK init was located by grep over specs/** and re-checked against the changed code.
>
> - **FR-027 (new) — BACKFILL (minor).** Same poller placement policy as dispatcher FR-011, including this branch's skip-first-two-cores-per-node change. `components/dispatcher-p2p/src/lib.rs:272-323, 894-903`. *Unspecced behaviour, modified by this branch.* Resolution: FR-027 added, cross-referencing dispatcher FR-011.
>
> No actionable drift remains for this component after apply.

> **Re-stamp 2026-10-01 (transitive: `components/interfaces` changed).** The spec-sync digest
> folds `components/interfaces/{src,specs}` into **every** component's hash, so an interface
> change invalidates all of them at once -- which is the design, not a defect. The interface
> change is the `served_by` attribution work: new `ServedBy` and `LookupOutcome` types, and
> `IDispatcher::batch_lookup` widened to return `Vec<LookupOutcome>`.
>
> **No re-analysis was performed for this component, and the digest bump asserts only what was
> actually checked**: the interface change is additive except for `batch_lookup`'s return type,
> which the compiler enforces across all implementors, and this component's own `src/**` and
> `specs/**` are unchanged. Components that implement `IDispatcher` (`dispatcher`,
> `dispatcher-p2p`) had their specs updated substantively; this one did not need it. If a later
> sweep finds drift here, this stamp is not evidence against it.

> **Re-stamp 2026-09-25 (SSD-evictor drive-selection bug fix; drift resolved by backfill).** Branch `fix/ssd-evictor-drive-index-hash`. The `BackgroundEvictor` (`src/background.rs`) freed the backing extent on the extent manager chosen by a raw `key % num_drives`, while write-through placement enqueues each `WriteJob` with `device_index = drive_index(key, num_drives)` (splitmix64, FR-018). For `num_drives > 1` the two disagree for almost all keys, so the evictor freed the wrong extent manager — leaking the intended extent and potentially freeing a live extent for another key on the wrong drive. CODE (authoritative): the evictor now routes through `drive_index` (made `pub(crate)`). SPEC BACKFILL: FR-019 was silent on the evictor's drive selection; a clause now states it uses the same `drive_index(key, num_drives)` placement hash as FR-018 and records the prior `key % num_drives` drift. `src/lib.rs` + `src/background.rs` changed (moved the digest); `spec.md` FR-019 changed (moved it again). Digest recomputed over a clean tree matching CI; drift status `clean`.

> **Re-stamp 2026-09-15 (workspace `cargo fmt` sweep; no drift).** Commit `f9bcd965` ("Add shmq RESERVE batch shared-deadline regression test") ran `cargo fmt` across the whole workspace, reflowing this component's `src/*.rs` (multi-line ↔ single-line argument lists and struct literals, import reordering). `git diff -w` confirms no token-level logic, signature, or contract change — the only substantive addition in that commit is a regression test in `lib/shmq-dispatcher/src/translate.rs`, which is outside this component and outside the spec-sync gate's `components/` scope. The formatting moved this component's `spec_sync_inputs_sha256`, but its spec↔implementation alignment is unchanged. Report body below stands unchanged; drift status remains `clean`. Digest recomputed over a clean tree matching CI.

> **Re-stamp 2026-09-15 (interface signature threaded; no drift).** Branch `fix-reserve-batch-deadline` (`bec6c6ec`) added a `deadline: Option<std::time::Instant>` parameter to `IDispatcher::reserve_memory`. This component's `src/` was touched only to thread that parameter through to satisfy the trait — it is ignored here (this dispatcher allocates straight-through / the mock is `unimplemented!()`), so there is no behavioral or contract change. This component's spec does not describe `reserve_memory`. Report body below stands unchanged; drift status remains `clean`. Digest recomputed over a clean tree matching CI.

> **Re-stamp 2026-09-09 (merged-branch interfaces-fold; no drift).** Branch
> `fix-dispatcher-store-backpressure` was merged into `unstable` at `3411518a`; that
> branch adds `DispatcherConfig::store_backpressure_ms` plus two `TierEventStats`
> counters (`store_backpressure_events`, `store_drops_on_full`) to
> `components/interfaces/src/idispatcher.rs`. `scripts/spec-sync-hash.sh` folds
> the whole `components/interfaces/` tree into every component's hash, so this
> component's digest moved even though its own `src/`+`specs/` are byte-for-byte
> unchanged and it references none of those new dispatcher symbols (verified by
> grep across `components/` and `lib/`). The merge's conflict resolution had
> reverted this stamp to unstable's `b220a1c8` value; the digest is recomputed
> here at merged HEAD. Drift status remains `clean`; the report body stands
> verbatim.

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
