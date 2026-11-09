# Spec-Sync Phase B — Proposals: eviction-policy-session-lists

**Generated**: 2026-08-20
**Based on**: `.specify/sync/drift-report.{json,md}` (1 drifted, 0 not_implemented, 0 unspecced; 23/24 aligned)
**Policy**: `.specify/sync/PHASE_B_POLICY.md` — classify by reading `location` code; spec-lag → BACKFILL, real bug → ALIGN.

Prior pass (`2026-08-07`, `proposals-20260807.json`) already resolved SC-001 by
downgrading it to a design goal and queued the harness as an align-task. This
pass re-verifies against the current code and reaffirms the BACKFILL.

## Proposal 1 — SC-001 (drifted, minor)

- **Requirement**: SC-001 — "On multi-turn traces, retain session-prefix blocks longer than LRU, reducing reloads by ≥15%."
- **Direction**: **BACKFILL** (spec → matches code)
- **Rationale**: Read `location` = `specs/001-session-list-eviction/spec.md:114` and the
  referenced source `benches/session_list_benchmark.rs`. The benchmark's four
  Criterion benches (`track`, `touch`, `batch_touch`, `identify_next_to_evict`)
  measure only this component's own hot-path throughput through the
  `IEvictionPolicy` interface. No comparative cross-policy (session-lists vs
  `eviction-policy-lru`) trace-replay / hit-rate harness exists anywhere in the
  repo. The eviction implementation itself is correct and intentional (all of
  FR-001..FR-018, SC-002..SC-006 aligned per drift-report). SC-001's ≥15%
  reload-reduction figure is therefore an unverified **aspirational design
  goal**, not a behavioral requirement the code violates. This is spec-lag /
  aspiration vs. reality, not a code bug → BACKFILL, not ALIGN. The spec was
  already downgraded on 2026-08-07; this pass keeps the requirement faithful to
  reality and sharpens it (names the exact benches, states it is explicitly not
  a measured outcome, points to the tracked follow-up).
- **Before**:
  > **SC-001** *(design goal — not yet verified; downgraded 2026-08-07)*: On representative multi-turn session traces, the policy is intended to retain session-prefix (head/interior) blocks longer than basic LRU, reducing re-loads of retained-lineage prefixes relative to basic LRU on the same trace and capacity (design target: ≥15%). This is an aspirational design goal pending a comparative trace-replay harness (session-lists vs `eviction-policy-lru` on a multi-turn ShareGPT-style trace, reporting prefix-reload counts); no such comparative benchmark currently exists in the repo — `benches/session_list_benchmark.rs` measures only this component's hot-path throughput, not a hit-rate comparison. Until that harness exists, the ≥15% figure is a target, not a measured outcome.
- **After**:
  > **SC-001** *(design goal — not a measured outcome; downgraded 2026-08-07, re-verified 2026-08-20)*: On representative multi-turn session traces, the policy is intended to retain session-prefix (head/interior) blocks longer than basic LRU, reducing re-loads of retained-lineage prefixes relative to basic LRU on the same trace and capacity (design target: ≥15%). This is an aspirational design goal, **not** a verified measurable outcome. Verifying it requires a comparative trace-replay harness (session-lists vs `eviction-policy-lru` on a multi-turn ShareGPT-style trace, reporting prefix-reload counts); no such comparative benchmark exists in the repo. `benches/session_list_benchmark.rs` covers only this component's own hot-path throughput — its four Criterion benches (`track`, `touch`, `batch_touch`, `identify_next_to_evict`) measure per-operation cost through `IEvictionPolicy` at scale, never a cross-policy hit-rate comparison. Until such a harness exists, the ≥15% figure remains a design target, not a measured outcome. Building that harness is tracked as a follow-up in `.specify/sync/align-tasks.md` (Task 1).

## Also applied

- Added a `**Last-Synced**: 2026-08-20` metadata line under the spec's `Status`
  header recording this Phase B re-sync.

## No other proposals

- **not_implemented**: none.
- **unspecced**: none.
- **ALIGN**: no new align tasks this pass. The pre-existing follow-up in
  `align-tasks.md` (Task 1 — build the comparative prefix-reload harness to
  eventually verify/retire the ≥15% target) remains valid and is carried
  forward unchanged. It is a verification TODO, not a behavioral-bug ALIGN.
