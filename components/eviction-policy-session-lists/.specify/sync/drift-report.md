# Spec ↔ Implementation Drift Report — eviction-policy-session-lists

**Generated**: pending

## Summary

| Metric | Count |
|--------|-------|
| Specs Analyzed | 1 |
| Requirements Checked | 24 (18 FR + 6 SC) |
| Aligned | 23 |
| Drifted | 1 |
| Not Implemented | 0 |
| Unspecced | 0 |

Component is effectively CLEAN. The single "drift" is a spec-acknowledged,
downgraded design goal (SC-001), not an implementation divergence.

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
| FR-008 select+remove oldest leaf / None when empty | `src/session_list.rs:190-193` |
| FR-009 parent promoted to leaf after eviction | `src/session_list.rs:161-174` |
| FR-010 up to N candidates, no removal | `candidates()` `src/session_list.rs:196-202` |
| FR-011 remove relinks interior child→parent | `unlink()` `src/session_list.rs:144-185` |
| FR-012 deterministic tie-break | monotonic `clock` + `(stamp,index)` total order `src/session_list.rs:62-65` |
| FR-013 len per pool | `src/lib.rs:215-221` |
| FR-014 clear pool | `src/lib.rs:223-228`; `src/session_list.rs:220-228` |
| FR-015 invalid handle/pool → error | `InvalidHandle`/`InvalidPool` `src/lib.rs:143-198` |
| FR-016 interface-only surface | all behavior via `IEvictionPolicy` impl `src/lib.rs:102-229` |
| FR-017 idempotent re-registration | `src/session_list.rs:91-94` |
| FR-018 single linear chain, one leaf | enforced by parent/child single links + `check_invariants` `src/session_list.rs:233-314` |
| SC-004 correctness (never non-leaf; oldest leaf; deterministic) | property tests `tests/lineage_properties.rs`, unit tests `src/session_list.rs:412-463` |
| SC-006 invariant consistency after any op sequence | `check_invariants` + randomized test `src/session_list.rs:514-564` |
| SC-002/003/005 perf targets | O(1)/O(log L) structure by construction (arena + BTreeSet); design-consistent |

Observability announce log (`src/lib.rs:107-120`) is documented in the spec's
Observability section — not unspecced.

### Drifted ⚠️

- **SC-001** (lineage retention ≥15% vs LRU) — **minor**. Spec text (line 114)
  already downgrades this to an unverified design goal and states the
  comparative trace-replay harness does not exist. Implementation is consistent
  with the spec's own acknowledgement; flagged only because the measurable
  outcome remains unverified. No code change implied.

### Not Implemented ✗

None.

## Unspecced Features

None.

## Recommendations

1. No source or spec change required. Component and spec are in sync.
2. When resources allow, build the session-lists-vs-LRU trace-replay harness to
   convert SC-001 from a design goal into a measured outcome.
