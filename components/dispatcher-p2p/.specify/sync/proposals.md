# Drift Resolution Proposals — dispatcher-p2p

Generated: 2026-09-02T21:32:13Z
Based on: `.specify/sync/drift-report.json` (cycle 2026-09-02)

## Summary

| Resolution Type | Count |
|-----------------|-------|
| Backfill (Code → Spec) | 0 |
| Align (Spec → Code) | 1 |
| Human Decision | 2 |
| New Specs | 0 |
| Remove from Spec | 0 |

The 2026-08-20 cycle already applied all backfills (SC-006 reword + FR-018..FR-023). This
cycle finds no new backfill: the spec is current. The one actionable drift (FR-017) remains
an ALIGN (code) item, and two items are surfaced for human decision.

## Proposals

### Proposal 1: 001-gpudirect-cold-path/FR-017

**Direction**: ALIGN

**Current State**:
- Spec says: eviction events dropped on a full/absent channel MUST be "silently dropped **and
  counted**", readable/resettable via `eviction_dropped_count()`.
- Code does: increments the counter only in dead-code `emit_eviction` (`src/lib.rs:228-236`,
  `#[allow(dead_code)]`, no callers). Live publish sites (`src/lib.rs:603-645`;
  `src/background.rs:414-419,611-616`) use bare `let _ = tx.try_send(...)` and never increment,
  so `eviction_dropped_count()` always returns 0.

**Proposed Resolution**: Fix code (do not modify spec). Route all live eviction publish sites
through a shared helper that increments `eviction_dropped` on `try_send` failure and when no
subscriber is registered; thread an `Arc<AtomicU64>` into `BackgroundEvictor::start` /
`MemoryTierEvictor::start`. Add a test that fills a capacity-1 channel and asserts a non-zero,
then zero-after-read, drop count. See `.specify/sync/align-tasks.md`.

**Rationale**: The spec requirement is a deliberate, agreed observability guarantee; the code has
the field and API but never feeds it. Spec is authoritative → align the code.

**Confidence**: HIGH

**Action**: `approved: true` (align task; no code edited by sync)

---

### Proposal 2: cold_staging_slots / cold_staging_buf_bytes

**Direction**: HUMAN_DECISION

**Current State**: `interfaces::DispatcherConfig` exposes `cold_staging_slots` (default 64) and
`cold_staging_buf_bytes` (default 4 MiB) at `components/interfaces/src/idispatcher.rs:84,87`, but
neither is referenced anywhere in `dispatcher-p2p/src/` (grep-verified). The 64-slot ring is
governed by FR-003's hard-coded `P2P_RING_SLOTS`.

**Proposed Resolution**: Human decides whether to (a) wire these fields into the ring allocation
(making FR-003 configurable) or (b) remove them from the config surface. Backfilling a functional
claim would invent behavior. **Out of scope for this sync**: the fields live in `interfaces/`,
which this workflow must not edit.

**Confidence**: n/a

**Action**: `approved: false` (pending human)

---

### Proposal 3: 001-gpudirect-cold-path/FR-022 vs FR-023 — lookup_async pin lifetime

**Direction**: HUMAN_DECISION

**Current State**: `lookup_async` releases the dispatch-map read pin at submission
(`src/lib.rs:2100`) before the caller synchronizes the returned `GpuStream`. FR-023 declares the
pin must outlive copy *completion* "for both the local hot-path async copy and the remote-lookup
delivery path"; the batch hot path (sync `src/lib.rs:1659` before release `1697`) and the remote
`PinnedKeys` path (sync `src/lib.rs:2026`) uphold it. `lookup_async` matches FR-022's explicit
caller-synchronization contract, so code and FR-022 agree while FR-023's scope claim is in tension.

**Proposed Resolution**: Human confirms whether `lookup_async`'s submission-time release is the
intended contract (in which case FR-023's scope wording could be narrowed) or a latent
demote-during-copy race that should adopt the `PinnedKeys` guard (a code ALIGN). No change applied
this cycle pending that decision.

**Confidence**: LOW (both readings plausible)

**Action**: `approved: false` (pending human)
