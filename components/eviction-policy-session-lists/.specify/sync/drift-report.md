---
spec_sync_component: eviction-policy-session-lists
spec_sync_drift_status: clean
spec_sync_synced_at: 2026-09-04T02:01:56Z
spec_sync_git_commit: 7343a2a8
spec_sync_inputs_sha256: cf85e57387bfc77b6b5699306279af654841ab780890e306a1c4ee1c6c7a7911
spec_sync_hash_tool: scripts/spec-sync-hash.sh
---
# Spec ↔ Implementation Drift Report — eviction-policy-session-lists

**This sweep (2026-09-03)** independently re-verified all 24 requirements
(18 FR + 6 SC) of `001-session-list-eviction` against `src/lib.rs` and
`src/session_list.rs`, and against the shared `IEvictionPolicy` interface
(`components/interfaces/src/ieviction_policy.rs`). One documentation drift was
found and fixed (FR-015 BACKFILL); no code drift.

## Summary

| Metric | Count |
|--------|-------|
| Specs Analyzed | 1 |
| Requirements Checked | 24 (18 FR + 6 SC) |
| Aligned | 23 |
| Drifted → resolved this sweep | 1 (FR-015 BACKFILL, doc) |
| Not Implemented | 0 |
| Unspecced | 0 |

**Verification this sweep** (workspace default member — fully buildable/testable):
- `cargo build -p eviction-policy-session-lists` — clean
- `cargo clippy -p eviction-policy-session-lists --all-targets -- -D warnings` — clean
- `cargo test -p eviction-policy-session-lists` — green (lib unit tests +
  `tests/lineage_properties.rs`)

## Resolved this sweep

**FR-015 — BACKFILL (spec doc).** The prior report marked FR-015 "Aligned" with
a blanket reading, but FR-015's text over-generalized. Original text:

> "Operations on an invalid or already-removed handle, or on a non-existent
> eviction domain, MUST be reported as errors rather than silently succeeding
> or corrupting state."

The shared `IEvictionPolicy` interface splits its methods into two classes:

- **Fallible, `Result`-returning:** `track` (`ieviction_policy.rs:87`), `touch`
  (`:92`), `batch_touch` (`:96`), `remove` (`:99`) — these return
  `Result<_, EvictionPolicyError>`.
- **Read-only queries with no error channel:** `identify_next_to_evict` →
  `Option<CacheKey>` (`:104`), `get_eviction_candidates` → `Vec<CacheKey>`
  (`:108`), `len` → `usize` (`:111`), `clear_pool` → `()` (`:114`).

The query methods **structurally cannot** report a pool-existence error — they
have no `Result`. `IEvictionPolicy` is a shared, multi-implementor trait
(`eviction-policy-lru` also implements it; consumed by dispatch-map/memory-tier),
so adding an error channel would be a breaking cross-cutting change nobody
requested. The implementation correctly makes the query methods **degrade
safely** on a non-existent pool: `identify_next_to_evict` returns `None`
(`src/lib.rs:200-205`, `?`-on-`get`), `get_eviction_candidates` returns an empty
`Vec` (`:207-213`), `len` returns `0` (`:215-221`), `clear_pool` is a no-op
(`:223-228`). The fallible methods correctly return `InvalidPool`/`InvalidHandle`
(`:143-198`); the test `invalid_pool_is_reported` (`:250-260`) pins both
behaviors.

This is a documentation over-reach against a deliberate, correct interface
contract — **not** a code bug — so it is a BACKFILL (fix the spec), never an
ALIGN. FR-015 was reworded to scope the "must report an error" requirement to
the fallible `Result`-returning methods, to state that the query methods carry
no error channel and MUST degrade safely on a non-existent domain, and to retain
the blanket "in no case may any operation corrupt tracking state."

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
| FR-015 fallible ops report error; query ops degrade safely | fallible: `InvalidHandle`/`InvalidPool` `src/lib.rs:143-198`; queries degrade `src/lib.rs:200-228`; test `invalid_pool_is_reported` `src/lib.rs:250-260` (see "Resolved this sweep") |
| FR-016 interface-only surface | all behavior via `IEvictionPolicy` impl `src/lib.rs:102-229` |
| FR-017 idempotent re-registration | `src/session_list.rs:91-94` |
| FR-018 single linear chain, one leaf | enforced by parent/child single links + `check_invariants` `src/session_list.rs:233-314` |
| SC-004 correctness (never non-leaf; oldest leaf; deterministic) | property tests `tests/lineage_properties.rs`, unit tests `src/session_list.rs:412-463` |
| SC-006 invariant consistency after any op sequence | `check_invariants` + randomized test `src/session_list.rs:514-564` |
| SC-002/003/005 perf targets | O(1)/O(log L) structure by construction (arena + BTreeSet); design-consistent |

Observability announce log (`src/lib.rs:107-120`) is documented in the spec's
Observability section — not unspecced.

### Drifted ⚠️

None actionable after this sweep.

- **SC-001** (lineage retention ≥15% vs LRU) — **not drift**. Spec text (line 116)
  already downgrades this to an aspirational design goal and records that the
  comparative trace-replay harness does not exist; the implementation is
  consistent with the spec's own acknowledgement. Building that harness is
  tracked in `.specify/sync/align-tasks.md` (Task 1). No code or spec change
  implied.

### Not Implemented ✗

None.

## Unspecced Features

None.

## Recommendations

1. No source change required. FR-015's wording is now consistent with the shared
   `IEvictionPolicy` contract; the component and spec are in sync.
2. When resources allow, build the session-lists-vs-LRU trace-replay harness to
   convert SC-001 from a design goal into a measured outcome (align-tasks Task 1).
