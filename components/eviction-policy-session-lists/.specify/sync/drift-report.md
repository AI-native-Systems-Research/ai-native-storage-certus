---
spec_sync_component: eviction-policy-session-lists
spec_sync_drift_status: drift
spec_sync_synced_at: 2026-09-02T21:29:33Z
spec_sync_git_commit: 2fc1cd3c
spec_sync_inputs_sha256: cc5a229fc1aae3f255d77414b2c535dce32e178242a47e9709d1c8ecd5a06313
spec_sync_hash_tool: scripts/spec-sync-hash.sh
---

# Spec ↔ Implementation Drift Report — eviction-policy-session-lists

**Generated**: 2026-09-02

## Summary

| Metric | Count |
|--------|-------|
| Specs Analyzed | 1 |
| Requirements Checked | 24 (18 FR + 6 SC) |
| Aligned | 23 |
| Drifted | 1 |
| Not Implemented | 0 |
| Unspecced | 0 |

The single drift is **SC-001**, and this pass **corrects** how it drifts. The
2026-08-07 and 2026-08-20 syncs both asserted "no comparative trace-replay
harness exists in the repo" and treated SC-001 as merely unverifiable. That
assertion is **false**: `apps/eviction-replay-benchmark` is exactly such a
harness and has existed since this component's introducing commit
(`c247b890`, 2026-08-04). SC-001 is therefore now **measurable**, and the
measurements available (in that app's README) show the ≥15% target is **not
met** on the sampled runs. The implementation (FR-001..FR-018, SC-002..SC-006)
is correct and intentional; only SC-001's spec text was stale. Direction:
BACKFILL.

## Spec: 001-session-list-eviction — Session-Lineage Eviction Policy

### Aligned ✓

| Req | Evidence |
|-----|----------|
| FR-001 register returns handle | `src/lib.rs:123-141` (`track` → `EvictionHandle::new`) |
| FR-002 parent = current leaf, new block becomes leaf | `src/session_list.rs:90-119` (`register`) |
| FR-003 independent per-session chains | `sessions: HashMap<SessionId,u32>` `src/session_list.rs:49,97,114` |
| FR-004 touch refreshes recency | `src/session_list.rs:123-137`; `src/lib.rs:143-155` |
| FR-005 batch refresh | `src/lib.rs:157-184` (`batch_touch`) |
| FR-006 no refresh on eviction | `evict_oldest`→`unlink` never ticks; `src/session_list.rs:190-193,144-185` |
| FR-007 only leaves evictable | `leaves: BTreeSet<(stamp,idx)>` `src/session_list.rs:51,196-202` |
| FR-008 select+remove oldest leaf / None when empty | `src/session_list.rs:190-193`; `src/lib.rs:200-205` |
| FR-009 parent promoted to leaf after eviction | `src/session_list.rs:161-174` |
| FR-010 up to N candidates, no removal | `candidates()` `src/session_list.rs:196-202`; `src/lib.rs:207-213` |
| FR-011 remove relinks interior child→parent | `unlink()` `src/session_list.rs:144-185` |
| FR-012 deterministic tie-break | monotonic `clock` + `(stamp,index)` total order `src/session_list.rs:62-65,50-51` |
| FR-013 len per pool | `src/lib.rs:215-221`; `src/session_list.rs:215-217` |
| FR-014 clear pool | `src/lib.rs:223-228`; `src/session_list.rs:220-228` |
| FR-015 invalid handle/pool → error | `InvalidHandle`/`InvalidPool` `src/lib.rs:130-137,143-198` |
| FR-016 interface-only surface | all behavior via `IEvictionPolicy` impl `src/lib.rs:102-229`; `Pool` is `pub(crate)` `src/session_list.rs:24,41` |
| FR-017 idempotent re-registration | `src/session_list.rs:91-94` (early return + `touch`) |
| FR-018 single linear chain, one leaf | single `parent`/`child` links `src/session_list.rs:30-32`; enforced by `check_invariants` `src/session_list.rs:279-313` |
| SC-002 O(1) register/touch/remove | arena + free list + hashmap; `alloc`/`touch`/`unlink` are index ops `src/session_list.rs:62-137,144-212` |
| SC-003 victim selection scales with #sessions | `BTreeSet` first-element pop `src/session_list.rs:190-193` (O(log L), L = #leaves = #non-empty sessions) |
| SC-004 correctness (never non-leaf; oldest leaf; deterministic) | unit tests `src/session_list.rs:412-463`; black-box reference-model property test `tests/lineage_properties.rs` |
| SC-005 recency-refresh sustains hot path | `batch_touch` single lock acquisition `src/lib.rs:157-184`; `benches/session_list_benchmark.rs` `batch_touch` bench |
| SC-006 invariant consistency after any op sequence | `check_invariants` `src/session_list.rs:233-314` + randomized test `src/session_list.rs:514-564`; `tests/lineage_properties.rs` |

Observability announce log (`src/lib.rs:107-120`) is documented in the spec's
`Observability` section (`spec.md:136-145`) — not unspecced.

### Drifted ⚠️

- **SC-001** (lineage retention vs LRU, ≥15% design target) — **moderate**.
  The spec text (as of the 2026-08-20 pass) asserted that *no* comparative
  trace-replay harness exists and that SC-001 was consequently unverifiable.
  **This is factually wrong.** `apps/eviction-replay-benchmark` (workspace
  member; `Cargo.toml:43,75`) replays a real multi-turn Qwen-Bailian trace
  through **both** `eviction-policy-session-lists` and `eviction-policy-lru`
  and reports per-policy hit-rate (a miss == a reload) at configurable cache
  sizes:
  - `apps/eviction-replay-benchmark/src/main.rs` — CLI, `--policy both`,
    `--cache-size N[,N…]`, side-by-side table.
  - `apps/eviction-replay-benchmark/src/sim.rs:148-206` — fixed-capacity cache
    simulator over `IEvictionPolicy`; hit == `touch`, miss == evict-then-`track`.
  - `apps/eviction-replay-benchmark/src/replay.rs:117-144` — derives each
    request's `session_id` as its conversation root (transitive
    `parent_chat_id`), giving lineage-aware policies the full multi-turn chain.
  - `apps/eviction-replay-benchmark/tests/replay_hits.rs` — offline tests over
    both policies (root resolution, exact no-eviction hit count, LRU
    monotonicity, bookkeeping invariants, latency-metric population).
  - First appeared in commit `c247b890` ("Add eviction-replay-benchmark and
    session-lists docs/logging") — the **same** commit that introduced this
    component. The harness has existed the entire time.

  Consequence: SC-001 is **measurable**, not unverifiable. The measured effect
  (the app README's sampled runs) is real but **workload- and cache-size
  dependent and below the ≥15% target** — e.g. `chat` trace at cache 256:
  session-lists 8.2% vs LRU 7.2% hit-rate; the gap narrows as the cache
  approaches the working set and LRU can edge ahead once eviction is rare.
  SC-001 has been BACKFILLED to state this accurately and to point at the
  existing harness. No code change is implied; the eviction implementation is
  correct.

### Not Implemented ✗

None.

## Unspecced Features

None **within this component's `src/`**. The comparative harness
`apps/eviction-replay-benchmark` is a separate application, not part of this
component; it is called out above because it is the artifact SC-001's
verification depends on and which prior syncs missed.

## Recommendations

1. No source or spec change required in the component. The eviction code is in
   sync with all functional/behavioural requirements.
2. Refine SC-001 from an aspirational ≥15% target into a concrete measured
   criterion by choosing a representative workload/capacity operating point in
   `apps/eviction-replay-benchmark` (align-tasks.md Task 1, now narrowed —
   the harness itself is already built).
