# Drift Report: eviction-policy-lru

Generated: 2026-08-07T15:28:26Z

Spec: `.specify/specs/001-lru-eviction-policy/spec.md` (Status: Backfilled)
Implementation: `src/lib.rs`, `src/lru_list.rs`
Interface: `components/interfaces/src/ieviction_policy.rs`

## Summary

| Class | Count |
|-------|-------|
| Aligned | 20 |
| Drifted | 0 |
| Not Implemented | 0 |
| Unspecced | 0 |

Result: **clean**. Every functional and non-functional requirement and success
criterion maps to implementing code. This matches the recent commit
`8a3ebc3f docs(eviction-policy-lru): fix FR-002 track signature drift`, which
resolved the last known divergence (the `semantics: BlockSemantics` argument).

## Detailed Findings

### Functional Requirements

| ID | Status | Location |
|----|--------|----------|
| FR-001 create_pool sequential from 0 | Aligned | `src/lib.rs:41-51` |
| FR-002 track(pool,key,semantics), ignores semantics | Aligned | `src/lib.rs:53-71` (`_semantics` unused) |
| FR-003 touch O(1) → MRU | Aligned | `src/lib.rs:73-87`; `lru_list.rs:70-96` |
| FR-004 remove O(1) unlink | Aligned | `src/lib.rs:117-131`; `lru_list.rs:121-145` |
| FR-005 identify_next_to_evict removes+returns LRU or None | Aligned | `src/lib.rs:133-138`; `lru_list.rs:99-104` |
| FR-006 get_eviction_candidates(n) no removal | Aligned | `src/lib.rs:140-149`; `lru_list.rs:107-118` |
| FR-007 len(pool) | Aligned | `src/lib.rs:151-160` |
| FR-008 clear_pool(pool) | Aligned | `src/lib.rs:162-168`; `lru_list.rs:148-154` |
| FR-009 Result methods → InvalidPool; Option/scalar degrade | Aligned | `src/lib.rs:60-67, 75-83, 119-127, 135, 142-147, 153-158, 164` |
| FR-010 touch/remove idempotent on stale handle, Ok(()); InvalidHandle unused | Aligned | `lru_list.rs:71-73, 122-124` (active flag) |
| FR-011 free-list node recycling | Aligned | `lru_list.rs:21, 38-45, 143` |
| FR-012 batch_touch amortizes lock | Aligned | `src/lib.rs:89-115` |

### Non-Functional Requirements

| ID | Status | Location |
|----|--------|----------|
| NFR-001 O(1) single-entry ops | Aligned | index-based DLL, `lru_list.rs` |
| NFR-002 thread-safe, no corruption | Aligned | `RwLock`+per-pool `Mutex`; test `concurrent_access` `src/lib.rs:308-336` |
| NFR-003 per-pool locking granularity | Aligned | `Vec<Mutex<Pool>>` behind `RwLock` `src/lib.rs:22-25` |
| NFR-004 conforms to component model, provides IEvictionPolicy, ILogger receptacle | Aligned | `define_component!` `src/lib.rs:27-38` |

### Success Criteria

| ID | Status | Notes |
|----|--------|-------|
| SC-001 lib.rs + lru_list.rs tests pass | Aligned | 10 tests in `lib.rs`, 13 in `lru_list.rs` |
| SC-002 4×100 concurrent, no corruption | Aligned | `concurrent_access` `src/lib.rs:308-336` |
| SC-003 clippy -D warnings + fmt | Aligned (assumed; not re-run here) | — |
| SC-004 integrates via query_interface!/receptacle in consumers | Aligned (spec-declared consumers not re-verified) | — |

## Unspecced Code

| Item | Location | Notes |
|------|----------|-------|
| (none) | — | `peek_front_n` and `Node.active` are internal helpers already described in Implementation Notes; no public surface beyond the interface. |

## Recommendations

- No action required. Component and spec are synchronized.
- SC-003/SC-004 are asserted but not machine-checked in this analysis; the repo's
  CI gate already covers clippy/fmt per project conventions.
