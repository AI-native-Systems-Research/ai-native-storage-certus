# Align Tasks — eviction-policy-session-lists

Generated: 2026-08-07 (branch `sync/spec-drift-sweep-20260807`)
Based on: `.specify/sync/drift-report.{json,md}` (2026-08-07T08:32Z)

Non-HIGH ALIGN items are **queued, not drafted**, per sweep pacing.

## Task 1: Pin SC-001 to a measured operating point (harness already exists)

> **Update 2026-09-02 (spec-sync):** The comparative trace-replay harness this
> task called for **already exists** — `apps/eviction-replay-benchmark` — and
> has since this component's introducing commit (`c247b890`, 2026-08-04). It
> replays a real multi-turn Qwen-Bailian trace through both
> `eviction-policy-session-lists` and `eviction-policy-lru` at configurable
> cache sizes and reports per-policy hit-rate (a miss == a reload) plus hot-path
> latency, and ships offline tests (`tests/replay_hits.rs`). The "build the
> harness" portion of this task is therefore **DONE**. What remains is narrower:
> pick a representative workload/capacity operating point and rewrite SC-001 as
> a concrete measured criterion (the app's sampled results are
> workload/cache-size dependent and do **not** reach the original ≥15% target,
> so SC-001 must either restate the observed figure or justify a specific
> operating point). The 2026-08-07 and 2026-08-20 syncs missed this app because
> they only inspected `benches/session_list_benchmark.rs`.

**Severity**: Low (does not block correctness; SC-001 has been downgraded to a
design goal in the interim).

**Spec Requirement**: SC-001 — the policy is intended to retain
session-prefix (head/interior) blocks longer than basic LRU, reducing re-loads
of retained-lineage prefixes by a design target of ≥15% on a representative
multi-turn trace at the same capacity.

**Current State**: The comparative harness EXISTS — `apps/eviction-replay-benchmark`
replays a shared multi-turn Qwen-Bailian trace through both
`eviction-policy-session-lists` and `eviction-policy-lru` at configurable cache
sizes and reports per-policy hit-rate (miss == reload) plus hot-path latency
(`--policy both`, `--cache-size N[,N…]`). Its sampled README results show a
real but workload/cache-size-dependent effect that does **not** reach ≥15%.
`benches/session_list_benchmark.rs` remains an in-component micro-benchmark only.

**Required Change**: Using `apps/eviction-replay-benchmark`, select a
representative workload/capacity operating point and rewrite SC-001 as a
concrete measured criterion — either (a) an operating point where the
lineage-preserving gain is stated at its measured value, or (b) an explicit
statement of the observed cross-workload range. No new harness is needed.

**Files to Modify**: `specs/001-session-list-eviction/spec.md` SC-001 (rewrite
to the measured criterion); optionally extend `apps/eviction-replay-benchmark`
to emit a prefix-reload breakdown if a finer metric than aggregate hit-rate is
wanted.

**Estimated Effort**: medium.

### Acceptance Criteria
- [x] A runnable harness replays one shared trace through both policies at equal capacity. *(done: `apps/eviction-replay-benchmark`)*
- [x] It reports hit-rate (miss == reload) for each policy so the ≥15% target can be evaluated. *(done: `--policy both`)*
- [ ] SC-001 is updated to reflect the measured outcome (restated as a concrete measured criterion at a chosen operating point rather than an aspirational ≥15% target).
