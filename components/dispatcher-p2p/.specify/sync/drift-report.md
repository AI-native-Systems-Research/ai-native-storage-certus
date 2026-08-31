# Spec ↔ Implementation Drift Report: dispatcher-p2p

**Generated**: 2026-08-31 (re-analysis)
**Spec analyzed**: `specs/001-gpudirect-cold-path/spec.md` (Status: Draft, Feature Branch: `p2p_component`, Last-Synced 2026-08-20)
**Mode**: Read-only drift analysis (no build, no source modification).

## Summary

| Metric | Count |
|--------|-------|
| Specs Analyzed | 1 |
| Requirements Checked | 29 (FR-001…023, SC-001…006) |
| Aligned | 27 |
| Drifted (open ALIGN, code-side) | 1 (FR-017) |
| Not Implemented | 0 |
| Unspecced Code Features | 1 (new: `tier_event_stats()` zeroed stub) |
| Human Decision (carried) | 1 (`cold_staging_*` dead config) |

### Delta since the 2026-08-20 sync

Three commits touched `components/dispatcher-p2p/` since the last sync (all on `src/lib.rs`):

- **`4659626b`** *feat: emit KV tier-event counts for the profiler* — added a `tier_event_stats()` method to satisfy the new required `IDispatcher::tier_event_stats()` trait method (`../interfaces/src/idispatcher.rs:564`). In dispatcher-p2p it is a **deliberate zeroed stub**: `interfaces::TierEventStats::default()` with the comment "dispatcher-p2p does not track tier-movement counters; report zeroed" (`src/lib.rs:2665-2668`). → **new unspecced seam** (see below).
- **`495c5acc`** *fix: implement memcpy_batch_async in MockGpuServices* — added `memcpy_batch_async` to the **`#[cfg(test)]` MockGpuServices only** (`src/lib.rs:3424+`), keeping the test mock in step with the expanded `IGpuServices` trait (E0046 build fix). Test infrastructure; no component behavior and no interface claim of dispatcher-p2p's own → out-of-scope observation, same nature as the FR-015 "mock keep-up" note.
- **`3231f85c`** — merge commit (carries the above).

The 2026-08-20 backfills (SC-006 reword; FR-018…FR-023 for the background/admin/async features) are present and correct in the spec and remain aligned. The `tier_event_stats` stub is the only new code seam; the FR-017 ALIGN and the `cold_staging_*` HUMAN_DECISION are carried forward unchanged.

## Detailed Findings — 001-gpudirect-cold-path

### Aligned ✓

FR-001…FR-016 and FR-018…FR-023, plus SC-001…SC-005, match the implementation (unchanged from the 2026-08-20 report; see prior evidence in `apply-report.md`). Re-confirmed this run:

- **SC-006** (init logs a non-fatal diagnostic and continues; panic deferred to first cold `batch_lookup`; single-key `lookup()` DRAM fallback) — now consistent with FR-006/FR-007/US2 after the 2026-08-20 reword. `src/lib.rs:1209-1213`, `:1752-1755`.
- **FR-018…FR-023** (ParallelBackgroundWriter, BackgroundEvictor, MemoryTierEvictor, `clear_memory_tier()`, `lookup_async()`, `PinnedKeys`) — backfilled 2026-08-20; aligned.
- **FR-008** (drop-in `IDispatcher` parity) — still satisfied; the trait surface grew a `tier_event_stats()` method, which dispatcher-p2p now implements (as a zeroed stub — see Unspecced below). No parity gap.

### Drifted ⚠️

- **FR-017 — eviction drop-count never incremented** — *moderate*. **STILL OPEN (code-side ALIGN, carried from 2026-08-20).**
  - Spec: a dropped `EvictionEvent` "MUST be silently dropped **and counted**, and the running drop count MUST be readable and reset via `eviction_dropped_count()`."
  - Actual: `eviction_dropped.fetch_add` is incremented **only** in `emit_eviction` (`src/lib.rs:229-235`), which is `#[allow(dead_code)]` with **no call sites**. All six live publish sites still discard the `Err` via bare `let _ = tx.try_send(...)`: `src/lib.rs:603, 619, 634, 641` and `src/background.rs:415, 612`. `eviction_dropped_count()` (`src/lib.rs:224-226`) therefore always returns 0.
  - Direction: **ALIGN** (spec correct; code defect). Requires `src/**` edits — outside this sync's editable scope. Retained in `align-tasks.md`.
  - Location: `src/lib.rs:224-236, 603-645`; `src/background.rs:415, 612`.

### Not Implemented ✗

None.

## Unspecced Code Features

| Feature | Location | Suggested Spec | Direction |
|---------|----------|----------------|-----------|
| `tier_event_stats()` — required `IDispatcher` trait method, implemented as a **zeroed stub** (`TierEventStats::default()`) because dispatcher-p2p does no tier-movement tracking (the profiler counter subsystem lives in the standard dispatcher, FR-058). | `src/lib.rs:2665-2668` (trait method at `../interfaces/src/idispatcher.rs:564`) | Amend FR-008 (interface-parity) to note that the standard-dispatcher interface now includes `tier_event_stats()`, which dispatcher-p2p satisfies as a zeroed stub. | **BACKFILL** |

## Carried Human Decision (unresolved)

| Item | Source | Status |
|------|--------|--------|
| `cold_staging_slots` / `cold_staging_buf_bytes` config fields | `../interfaces/src/idispatcher.rs:84,87` (defaults `:109,110`) | **STILL DEAD** on this component's surface — no references anywhere in `dispatcher-p2p/src/` (grep-verified). The 64-slot ring is governed by FR-003 (`P2P_RING_SLOTS`), not these fields. Resolution (wire in or remove from the config surface) requires an `interfaces/**` + `src/**` change, outside sync scope. Carried. |

## Out-of-Scope Observations (informational — not editable by this sync)

- **`memcpy_batch_async` in MockGpuServices** (`src/lib.rs:3424+`, `#[cfg(test)]`) — test mock keeping up with the expanded `IGpuServices` trait. No component behavior; no requirement needed. Analogous to the FR-015 "mock-only keep-up" note already in the spec.
- **`plan.md` staleness (minor, carried)** — Source-Code layout omits `cold_pool.rs`/`pins.rs`; bench/test names don't match the tree. `plan.md` is a spec-kit artifact under `specs/**` but the staleness is a doc-refresh item; not a requirement drift.

## Recommendations

1. **BACKFILL FR-008** (spec edit, in scope): note the `tier_event_stats()` interface method and dispatcher-p2p's zeroed-stub implementation, so the drop-in-parity requirement reflects the current `IDispatcher` surface.
2. **FR-017** (code, out of scope): resolve via the retained align task — route all live eviction publishes through a shared helper that increments `eviction_dropped` on `try_send` failure.
3. **`cold_staging_*`** (human decision, out of scope): wire the fields into the ring sizing or remove them from the config surface.
