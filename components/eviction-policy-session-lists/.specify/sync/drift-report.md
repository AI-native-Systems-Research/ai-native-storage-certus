# Drift Report: eviction-policy-session-lists

Generated: 2026-08-07T15:28:26Z

Spec: `specs/001-session-list-eviction/spec.md` (Status: Draft)
Implementation: `src/lib.rs`, `src/session_list.rs`
Interface: `components/interfaces/src/ieviction_policy.rs` (shared, session-aware `track`)
Tests: `tests/lineage_properties.rs`; internal tests in `src/session_list.rs`
Bench: `benches/session_list_benchmark.rs`

## Summary

| Class | Count |
|-------|-------|
| Aligned | 22 |
| Drifted | 0 |
| Not Implemented | 1 (SC-001, unverified comparative outcome) |
| Unspecced | 1 (one-time startup info-log) |

Functional behavior is fully aligned. The only gaps are a success criterion that
requires a comparative measurement not present in the repo (SC-001), and a small
unspecced startup log line.

## Detailed Findings

### Functional Requirements — all Aligned

| ID | Requirement | Location |
|----|-------------|----------|
| FR-001 register block+session, return handle | `src/lib.rs:123-141`; `session_list.rs:90-119` |
| FR-002 new block's parent = session's current leaf; new block becomes leaf | `session_list.rs:96-115` |
| FR-003 one block ↔ one session; independent chains | `session_list.rs:49,97,114`; test `distinct_sessions_form_independent_chains` |
| FR-004 access refreshes recency timestamp | `session_list.rs:123-137` (`touch`) |
| FR-005 batch recency refresh | `src/lib.rs:157-184` (`batch_touch`) |
| FR-006 no recency refresh on eviction | `session_list.rs:190-193` (`evict_oldest`→`unlink`, no `tick`) |
| FR-007 only leaves are eviction-eligible | `session_list.rs:51,110,161-174` (`leaves` BTreeSet) |
| FR-008 identify victim = oldest leaf across sessions; remove+return; None if empty | `session_list.rs:190-193`; `src/lib.rs:200-205` |
| FR-009 after evict/remove parent becomes leaf | `session_list.rs:161-174` |
| FR-010 up to N candidates in order, no removal | `session_list.rs:196-202` (`candidates`) |
| FR-011 remove by handle; interior re-links child↔parent | `session_list.rs:144-185,206-212`; test `remove_interior_relinks_chain` |
| FR-012 deterministic tie-break | `session_list.rs:60-65` (strictly increasing `clock` ⇒ unique `(stamp,index)` keys, no ties) |
| FR-013 report count tracked | `session_list.rs:215-217` (`len`) |
| FR-014 clear domain to empty | `session_list.rs:220-228` (`clear`) |
| FR-015 invalid handle/domain → error | `src/lib.rs:143-155,186-198` (`InvalidHandle`/`InvalidPool`) |
| FR-016 behavior exposed only via IEvictionPolicy | `src/lib.rs:102-229` (no other public API; `Pool` is `pub(crate)`) |
| FR-017 re-registering tracked key is idempotent recency refresh | `session_list.rs:91-94`; test `reregister_is_idempotent_recency_refresh` |
| FR-018 single linear chain, ≤1 child, one leaf per non-empty session | `session_list.rs:30-36,110-114`; `check_invariants` |

### Success Criteria

| ID | Status | Notes |
|----|--------|-------|
| SC-001 ≥15% fewer prefix re-loads vs LRU on multi-turn traces | **Not Implemented (unverified)** | No comparative trace benchmark against `eviction-policy-lru` exists. `benches/session_list_benchmark.rs` measures only this component's hot-path throughput, not a hit-rate comparison. |
| SC-002 register/refresh/remove ~constant to ≥1M blocks | Aligned | Ops are O(log L) in active sessions L, independent of total blocks. Bench: `bench_track`, `bench_touch`, `bench_batch_touch` (`benches/session_list_benchmark.rs`). |
| SC-003 victim selection scales with #sessions, bounded at ≥1M blocks | Aligned | `evict_oldest` = `leaves.iter().next()` over BTreeSet keyed on leaves (≤ #sessions). Bench: `bench_identify_next_to_evict`. |
| SC-004 100% correct victim (never a node with a tracked child; oldest leaf; deterministic) | Aligned | `tests/lineage_properties.rs` shadow-model cross-check; unit tests `never_evicts_a_node_with_a_tracked_child`, `evicts_oldest_leaf_across_sessions`. |
| SC-005 recency-refresh sustains hot-path rate | Aligned | `bench_batch_touch` / `bench_touch` cover the batch-refresh path. |
| SC-006 lineage stays internally consistent after any op sequence | Aligned | `session_list.rs:233-314` `check_invariants` + randomized tests (`randomized_operations_preserve_invariants`, `tests/lineage_properties.rs`). |

## Unspecced Code

| Item | Location | Severity | Notes |
|------|----------|----------|-------|
| One-time "selected as active eviction policy" info log via `announced` flag | `src/lib.rs:83-87,107-120` | Low | Startup-announcement behavior + `EvictionState.announced` field not mentioned in spec. Operationally useful; consider a one-line note in spec Overview. |

## Cross-Component Note (not a drift of this component)

- `memory-tier` calls `track(pool, key, BlockSemantics::default())` (i.e.
  `session_id = 0`) for every block (`components/memory-tier/src/lib.rs:365-367`).
  If this policy is bound under memory-tier, all blocks collapse into one session
  chain, so only the most-recently-registered block is a leaf/eligible victim —
  degenerate lineage behavior. This is a memory-tier integration gap, recorded
  here for visibility; it is consistent with this component's spec (caller
  supplies the session id).

## Recommendations

- SC-001: add a trace-replay comparison harness (session-lists vs LRU on a
  multi-turn ShareGPT-style trace) reporting prefix re-load counts, or downgrade
  SC-001 to a design goal until such a harness exists. This is the one
  outstanding verification gap.
- Add a one-line spec note for the startup announcement log (Overview or an
  Observability subsection) to clear the sole unspecced item.
