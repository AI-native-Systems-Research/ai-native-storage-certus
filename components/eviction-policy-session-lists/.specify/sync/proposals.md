# Spec-Sync — Proposals: eviction-policy-session-lists

**Generated**: 2026-09-02
**Based on**: `.specify/sync/drift-report.{json,md}` (1 drifted, 0 not_implemented, 0 unspecced; 23/24 aligned)

This pass re-verified every FR/SC against the current code and, unlike the two
prior passes, discovered that the comparative harness assumed missing by SC-001
**already exists** (`apps/eviction-replay-benchmark`). SC-001's spec text is
therefore corrected, not merely reaffirmed.

## Proposal 1 — SC-001 (drifted, moderate) — BACKFILL, approved

- **Requirement**: SC-001 — "On multi-turn traces, retain session-prefix blocks
  longer than LRU, reducing reloads by ≥15% (design target)."
- **Direction**: **BACKFILL** (spec text → matches reality)
- **Rationale**: The eviction implementation is correct and intentional (all of
  FR-001..FR-018, SC-002..SC-006 aligned). The drift is entirely in SC-001's
  spec text, which asserted "no comparative benchmark exists in the repo" and
  that SC-001 was unverifiable. Reading the repo shows this is false:
  `apps/eviction-replay-benchmark` (workspace member, `Cargo.toml:43,75`) is a
  full comparative trace-replay harness — `src/sim.rs:148-206` layers a
  fixed-capacity cache over any `IEvictionPolicy`; `src/replay.rs:117-144`
  derives conversation-root `session_id`s from a real Qwen-Bailian multi-turn
  trace; `src/main.rs --policy both` runs session-lists and
  `eviction-policy-lru` side by side and reports per-policy hit-rate; and
  `tests/replay_hits.rs` exercises both offline. It first appeared in
  `c247b890` (2026-08-04) — the component's own introducing commit — so it has
  existed the whole time; the 2026-08-07 and 2026-08-20 syncs missed it by
  inspecting only `benches/session_list_benchmark.rs`. Because a factual claim
  in the spec about the repo's state is wrong (spec-lag, not a code bug), the
  fix is to correct the spec → BACKFILL, not ALIGN. Honesty requirement: the
  measured effect is real but workload/cache-size dependent and does **not**
  reach ≥15% on the sampled runs, so SC-001 is rewritten to say the target is
  measurable-but-unmet, not "verified".
- **Before** (excerpt):
  > **SC-001** *(design goal — not a measured outcome; downgraded 2026-08-07, re-verified 2026-08-20)*: … Verifying it requires a comparative trace-replay harness …; **no such comparative benchmark exists in the repo**. `benches/session_list_benchmark.rs` covers only this component's own hot-path throughput … Until such a harness exists, the ≥15% figure remains a design target … Building that harness is tracked as a follow-up in `.specify/sync/align-tasks.md` (Task 1).
- **After** (excerpt):
  > **SC-001** *(design goal — measurable via the comparative harness, but not met at the ≥15% target on sampled runs; … corrected 2026-09-02)*: … A comparative trace-replay harness **does** exist and measures exactly this: `apps/eviction-replay-benchmark` replays a real multi-turn LLM-serving trace … through both `eviction-policy-session-lists` and `eviction-policy-lru` … and reports per-policy cache hits / hit-rate … It has existed since this component's introducing commit and was overlooked by the 2026-08-07 and 2026-08-20 syncs … Measured results are **workload- and cache-size-dependent** and … do **not** reach the ≥15% figure … The ≥15% figure therefore remains an **aspirational design target that current measurements have not met**, not a verified outcome — but it is now measurable rather than unverifiable. Refining SC-001 to a specific measured criterion … is tracked as a follow-up in `.specify/sync/align-tasks.md` (Task 1).

## Proposal 2 — metadata (BACKFILL, approved)

- Updated the spec's `**Last-Synced**` line to `2026-09-02`, noting the
  correction (harness exists as `apps/eviction-replay-benchmark`).

## Proposal 3 — align-tasks.md Task 1 (re-scope, approved)

- Task 1 previously read "Build a comparative prefix-reload trace-replay
  harness". The harness is already built, so Task 1 is re-scoped to "Pin SC-001
  to a measured operating point" — the "build the harness" acceptance criteria
  are marked done; the remaining criterion is rewriting SC-001 to a concrete
  measured figure. (This edits `.specify/sync/align-tasks.md`, not code.)

## No other proposals

- **not_implemented**: none.
- **unspecced** (within component `src/`): none.
- **ALIGN**: none. No source change is warranted — the code satisfies every
  functional/behavioural requirement. Task 1 is a spec-refinement follow-up,
  not a behavioural-bug ALIGN.
