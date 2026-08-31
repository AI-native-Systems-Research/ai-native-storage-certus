# Spec Sync Proposals — dispatcher

Generated: 2026-08-31
Spec: components/dispatcher/specs/001-dispatcher-cache-interface/spec.md
Drift source: components/dispatcher/.specify/sync/drift-report.{json,md} (2 drifted, 0 not-implemented, 1 unspecced)
Mode: --interactive (all three proposals approved by the user)

Every drift item was classified by reading the code at its `location`. The code is
the working, intentional reality in every case; no code change is proposed (no ALIGN
items). All three are BACKFILL (spec → matches code).

---

## BACKFILL-FR040 — gRPC → shmq control transport

- **Direction**: BACKFILL (spec → matches code)
- **Requirement**: FR-040
- **Drift ref**: FR-040 (moderate)
- **Location**: components/dispatcher/src/lib.rs:44 (module doc), :3060 (`promote_to_memory_tier`)
- **Approval**: APPROVED

**Before** — *"The gRPC handler spawns this as a detached background task when `BatchTouchRequest.promote = true`."*

**After** — *"The shmq serve layer spawns this as a detached background task on the control-plane promote request (gRPC and the `BatchTouchRequest` message were removed in commit `97e26738`; shm-queue is the sole control transport)."*

**Rationale** — gRPC was removed ("Remove gRPC; make shm-queue the sole control transport",
`97e26738`, 2026-08-18). Control-plane requests now arrive from the shmq serve layer
(`src/lib.rs:5`, `:44`), and `BatchTouchRequest` no longer exists in the dispatcher source.
The `promote_to_memory_tier` method itself is unchanged. Spec-lag → BACKFILL.

---

## BACKFILL-FR042 — gRPC TakeEvents → shmq serve layer

- **Direction**: BACKFILL (spec → matches code)
- **Requirement**: FR-042
- **Drift ref**: FR-042 (minor)
- **Location**: components/dispatcher/src/lib.rs:442
- **Approval**: APPROVED

**Before** — *"This mechanism enables external consumers (e.g., gRPC TakeEvents stream) to observe cache evictions without polling."*

**After** — *"This mechanism enables external consumers (the shmq serve layer's `TakeEvents` drain) to observe cache evictions without polling."*

**Rationale** — The eviction channel is now drained by the shmq serve layer via `TakeEvents`
(`src/lib.rs:442`: "Returns the receiver that the shmq serve layer should drain via `TakeEvents`").
The `create_eviction_channel` / `eviction_dropped_count` mechanism is unchanged. Spec-lag → BACKFILL.

---

## BACKFILL-UNSPECCED-058 — Tier-event counters (new FR-058 + SC-017)

- **Direction**: BACKFILL-UNSPECCED (add new requirement to existing spec)
- **Requirement**: NEW FR-058 (+ SC-017); FR-001 inventory amended
- **Drift ref**: unspecced (`tier_event_stats` / `TierEventCounters`)
- **Location**: components/interfaces/src/idispatcher.rs:564 (trait method), :191 (`TierEventStats`);
  components/dispatcher/src/lib.rs:111-159 (`TierEventCounters`), :3390 (impl)
- **Approval**: APPROVED

**Before** — No requirement. FR-001's introspection inventory lists `read_write_stats` but not
`tier_event_stats`; no FR covers the tier-event counter subsystem.

**After** — Add FR-058 documenting the `tier_event_stats() -> TierEventStats` `IDispatcher` method
and the `TierEventCounters` subsystem: four monotonic (cumulative-since-process-start) counters —
promotions SSD→DRAM (`promotions_to_memory`), lookups served→GPU (`promotions_to_gpu`), memory-tier
evictions (`evictions_from_memory`), and SSD-extent evictions (`evictions_from_ssd`) — shared behind
an `Arc` so foreground dispatcher paths and background evictor threads bump the same counters;
`snapshot()` reads without reset (callers derive per-interval deltas by subtracting successive
snapshots); always populated (unlike telemetry-gated `read_write_stats`). Add `tier_event_stats` to
FR-001's durability/introspection method list. Add SC-017 as its measurable outcome.

**Rationale** — The method and counter subsystem ship and are committed (profiler telemetry:
"emit KV tier-event counts for the profiler", `4659626b`/`3231f85c`). Counters are recorded at
~11 sites across `lib.rs` and `background.rs`. Code is authoritative → backfill.

---

## Summary

| Proposal | Direction | Approved | Applied |
|---|---|---|---|
| BACKFILL-FR040 | BACKFILL | Yes | Yes (spec.md) |
| BACKFILL-FR042 | BACKFILL | Yes | Yes (spec.md) |
| BACKFILL-UNSPECCED-058 | BACKFILL-UNSPECCED | Yes | Yes (spec.md: FR-058 + SC-017, FR-001 amended) |

No ALIGN, RESOLVED, or HUMAN_DECISION items this run.

## Out-of-scope observations (recorded, not proposed as spec edits)

- Two stale "gRPC handler" mentions remain in `src/lib.rs` code comments (`:2983`, `:3016`) —
  source comments, outside this sync's editable scope. Suggested follow-up: reword to "shmq serve
  layer / null-stream caller".
- `components/dispatcher/verif/` reappeared as untracked Creusot build state; the interface makes
  no verification claims, so there is nothing to reconcile.
