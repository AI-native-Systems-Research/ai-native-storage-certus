# Align Tasks — eviction-policy-session-lists

Generated: 2026-08-07 (branch `sync/spec-drift-sweep-20260807`)
Based on: `.specify/sync/drift-report.{json,md}` (2026-08-07T08:32Z)

Non-HIGH ALIGN items are **queued, not drafted**, per sweep pacing.

## Task 1: Build a comparative prefix-reload trace-replay harness (verify or retire SC-001)

**Severity**: Low (does not block correctness; SC-001 has been downgraded to a
design goal in the interim).

**Spec Requirement**: SC-001 — the policy is intended to retain
session-prefix (head/interior) blocks longer than basic LRU, reducing re-loads
of retained-lineage prefixes by a design target of ≥15% on a representative
multi-turn trace at the same capacity.

**Current State**: No comparative benchmark exists.
`benches/session_list_benchmark.rs` measures only this component's own hot-path
throughput (register / access / evict), not a hit-rate comparison against
`eviction-policy-lru`. SC-001's ≥15% figure was therefore unverifiable and has
been downgraded to a stated design target (see `apply-report.md`, 2026-08-07).

**Required Change**: Add a trace-replay harness that runs both
`eviction-policy-session-lists` and `eviction-policy-lru` over a shared
multi-turn (ShareGPT-style) access trace at a fixed capacity, and reports
prefix-reload counts / hit-rate for each. Once measured, either (a) confirm the
≥15% target and restore SC-001 to a measured criterion, or (b) update SC-001 to
the observed figure.

**Files to Modify**: new `benches/` or `tests/` harness under
`components/eviction-policy-session-lists/`; a representative trace fixture;
`specs/001-session-list-eviction/spec.md` SC-001 (once measured).

**Estimated Effort**: medium.

### Acceptance Criteria
- [ ] A runnable harness replays one shared trace through both policies at equal capacity.
- [ ] It reports prefix-reload / hit-rate for each policy so the ≥15% target can be evaluated.
- [ ] SC-001 is updated to reflect the measured outcome (restored as a criterion or re-stated to the observed value).
