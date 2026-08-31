# Spec Sync Proposals
Generated: 2026-08-20 (Spec-Sync Phase B)
Project: dispatcher-p2p
Spec: 001-gpudirect-cold-path
Source: `.specify/sync/drift-report.json`

Summary: 1 ALIGN, 1 BACKFILL (drifted requirement), 6 BACKFILL-UNSPECCED, 1 HUMAN_DECISION.

---

## Proposal 1 — FR-017 (eviction drop-count guarantee unmet)

- **Direction**: ALIGN (spec correct, code buggy — no code change in this pass; task recorded)
- **Requirement**: FR-017
- **Rationale**: FR-017 mandates that undeliverable eviction events are dropped **and counted**,
  readable/resettable via `eviction_dropped_count()`. The counter is only touched in the
  dead-code `emit_eviction` (`src/lib.rs:228-236`, `#[allow(dead_code)]`, no callers). All live
  publish sites (`src/lib.rs:602-645`, `src/background.rs:414-419`, `:611-616`) use bare
  `let _ = tx.try_send(...)` and never increment the counter, so `eviction_dropped_count()` always
  returns 0. This is a genuine behavioral bug against a correct spec → ALIGN, not backfill.
- **Before**: (spec unchanged)
- **After**: (spec unchanged) — see `align-tasks.md` for the code-fix task.

---

## Proposal 2 — SC-006 (init panic vs. graceful init + deferred panic)

- **Direction**: BACKFILL (code authoritative — spec wording stale)
- **Requirement**: SC-006
- **Rationale**: SC-006 asserted initialization *panics* on P2P ring alloc failure. The code
  instead logs a non-fatal diagnostic at init (`src/lib.rs:1209-1213`) and defers the panic to the
  first cold `batch_lookup` (`.expect(...)`, `src/lib.rs:1752-1755`); single-key `lookup()`
  silently falls back to DRAM. This graceful-init/deferred-panic behavior is intentional and is
  already the agreed contract in FR-006, FR-007, and User Story 2 AC-1 — SC-006 was the lone stale
  statement. The drift report itself recommends rewording SC-006. Backfill SC-006 to match.
- **Before**:
  > **SC-006**: Initialization panics with a clear diagnostic when P2P ring allocation fails (GDRCopy/BAR1 unavailable).
- **After**:
  > **SC-006**: When P2P ring allocation fails (GDRCopy/BAR1 unavailable), initialization logs a clear, non-fatal diagnostic and continues (permitting hot-only testing without P2P hardware). The failure is surfaced fatally on first use: the first cold `batch_lookup` panics with a diagnostic directing the operator to the `full.yaml` profile, while the single-key `lookup()` path silently falls back to the DRAM path. (Consistent with FR-006, FR-007, and User Story 2 AC-1.)

---

## Proposal 3 — FR-018 (parallel write-through, one thread per drive) [UNSPECCED]

- **Direction**: BACKFILL-UNSPECCED (new FR)
- **Rationale**: `ParallelBackgroundWriter` (`src/background.rs:154-219`, started at
  `src/lib.rs:1287`) provides concurrent memory-tier→SSD write-through, routing each `WriteJob` to
  its target drive's dedicated writer thread, with in-flight accounting, `flush()`, and draining
  `shutdown()`. The spec was cold-read-only and did not describe the persistence path.
- **Before**: (none — feature unspecced)
- **After**: New **FR-018** (see spec.md) + Key Entity `ParallelBackgroundWriter` + User Story 5 AS-3.

---

## Proposal 4 — FR-019 (SSD capacity reclamation) [UNSPECCED]

- **Direction**: BACKFILL-UNSPECCED (new FR)
- **Rationale**: `BackgroundEvictor` (`src/background.rs:303-488`, started at `src/lib.rs:1400`
  when `ssd_eviction_threshold > 0.0`) periodically reclaims SSD capacity by evicting oldest keys
  down to a low watermark, freeing extents and emitting `Removed` events. Config
  `ssd_eviction_{threshold,low_watermark,batch_size,interval_secs}`.
- **Before**: (none — feature unspecced)
- **After**: New **FR-019** (see spec.md) + Key Entity `BackgroundEvictor` + User Story 5 AS-2.

---

## Proposal 5 — FR-020 (proactive DRAM→SSD demotion) [UNSPECCED]

- **Direction**: BACKFILL-UNSPECCED (new FR)
- **Rationale**: `MemoryTierEvictor` (`src/background.rs:490-654`, started at `src/lib.rs:1428`
  when `memory_tier_eviction_threshold > 0.0`) proactively demotes LRU DRAM entries to SSD with
  pressure-scaled batch sizing and dry-run backoff, emitting `Demoted` events. Config
  `memory_tier_eviction_*`. FR-017 covered only the eviction *event*, not the sweep.
- **Before**: (none — feature unspecced)
- **After**: New **FR-020** (see spec.md) + Key Entity `MemoryTierEvictor` + User Story 5 AS-1.

---

## Proposal 6 — FR-021 (clear_memory_tier admin op) [UNSPECCED]

- **Direction**: BACKFILL-UNSPECCED (new FR)
- **Rationale**: `clear_memory_tier()` (`src/lib.rs:2606-2637`) flushes the entire memory tier,
  demoting each entry to its SSD copy or force-removing it, and returns the cleared count.
- **Before**: (none — feature unspecced)
- **After**: New **FR-021** (see spec.md) + User Story 5 AS-4.

---

## Proposal 7 — FR-022 (lookup_async / caller-side stream) [UNSPECCED]

- **Direction**: BACKFILL-UNSPECCED (new FR)
- **Rationale**: `lookup_async()` (`src/lib.rs:2044-2110`) returns a `GpuStream` after issuing an
  async H2D copy on a warm CUDA stream (sync fallback if none), letting the caller pipeline
  hot-path synchronization. The single-key contract was undocumented in the spec.
- **Before**: (none — feature unspecced)
- **After**: New **FR-022** (see spec.md) + User Story 5 AS-5.

---

## Proposal 8 — FR-023 (read-pin lifetime guard across async copy) [UNSPECCED]

- **Direction**: BACKFILL-UNSPECCED (new FR)
- **Rationale**: `pins::PinnedKeys` (`src/pins.rs:26-57`, used at `src/lib.rs:1931`) holds
  dispatch-map read pins across the *completion* of an async H2D copy (not just submission),
  releasing them exactly once on drop across all exit paths. A leaked pin makes its entry
  permanently unevictable — a data-plane correctness invariant worth recording.
- **Before**: (none — feature unspecced)
- **After**: New **FR-023** (see spec.md) + Key Entity `PinnedKeys`.

---

## Proposal 9 — cold_staging_slots / cold_staging_buf_bytes config fields [HUMAN_DECISION]

- **Direction**: HUMAN_DECISION (not backfilled)
- **Rationale**: `cold_staging_slots` / `cold_staging_buf_bytes` are defined on the shared
  `interfaces::DispatcherConfig` (`components/interfaces/src/idispatcher.rs:81-87`) but are
  **not referenced anywhere in `components/dispatcher-p2p/src/`** (verified by grep). They are
  dead config on this component's surface. Writing a functional requirement asserting
  dispatcher-p2p honors these fields would be inventing behavior the code does not have (the
  fixed 64-slot P2P ring is governed by FR-003, not by these fields). Per Phase B policy
  ("don't invent"; HUMAN_DECISION when genuinely ambiguous), this is left for a human to decide:
  either wire the fields into the cold path, or remove them from the shared config surface. No
  spec edit made.
- **Before**: (none)
- **After**: (none — pending human decision)
