---
spec_sync_component: eviction-policy-lru
spec_sync_drift_status: drift
spec_sync_synced_at: 2026-09-02T21:32:00Z
spec_sync_git_commit: 2fc1cd3c
spec_sync_inputs_sha256: 2ecc142ee3d130b9f6ae93e9b5f2cb0381028064beb55516994645a8e5d837b3
spec_sync_hash_tool: scripts/spec-sync-hash.sh
---

# Drift Report: eviction-policy-lru

Generated: 2026-09-02T21:32:00Z

Spec: `.specify/specs/001-lru-eviction-policy/spec.md` (Status: Backfilled)
Plan: `.specify/specs/001-lru-eviction-policy/plan.md`
Implementation: `src/lib.rs`, `src/lru_list.rs`
Interface: `components/interfaces/src/ieviction_policy.rs`

> **Spec-location quirk**: This component's Spec Kit tree lives under
> `components/eviction-policy-lru/.specify/specs/001-lru-eviction-policy/`, not
> under a top-level `specs/`. Consequently `scripts/spec-sync-hash.sh` (which
> walks only `<dir>/src` + `components/interfaces/{src,specs}`) hashes the source
> and interface trees but **not** this spec. The digest above is therefore
> invariant to spec/plan edits and changes only when `src/**` or the interfaces
> tree changes — this is expected for this component and matches the tool's
> documented input set.

## Summary

| Class | Count |
|-------|-------|
| Aligned | 18 |
| Drifted | 2 |
| Not Implemented | 0 |
| Unspecced / Human Decision | 1 |

Result: **drift**. All functional/non-functional requirements and success
criteria are implemented and correct, tests/clippy/fmt pass. Two low-severity
documentation drifts were found and backfilled into the spec/plan; one
interface-vs-implementation divergence is left for human decision (interfaces
are out of scope to edit here).

## Verification performed

- `cargo test -p eviction-policy-lru` — **22 passed** (9 in `lib.rs`, 13 in `lru_list.rs`).
- `cargo clippy -p eviction-policy-lru -- -D warnings` — **clean**.
- `cargo fmt -p eviction-policy-lru -- --check` — **clean**.
- Consumer wiring verified via Cargo manifests + `EvictionPolicyLruComponent`/`query_interface!` usage.

## Detailed Findings

### Functional Requirements

| ID | Status | Location |
|----|--------|----------|
| FR-001 create_pool sequential from 0 | Aligned | `src/lib.rs:41-51` |
| FR-002 track(pool,key,semantics), ignores semantics | Aligned | `src/lib.rs:53-71` (`_semantics` unused; `push_back` = MRU) |
| FR-003 touch O(1) → MRU | Aligned | `src/lib.rs:73-87`; `lru_list.rs:70-96` |
| FR-004 remove O(1) unlink | Aligned | `src/lib.rs:117-131`; `lru_list.rs:121-145` |
| FR-005 identify_next_to_evict removes+returns LRU or None | Aligned | `src/lib.rs:133-138`; `lru_list.rs:99-104` |
| FR-006 get_eviction_candidates(n) no removal, O(n) | Aligned | `src/lib.rs:140-149`; `lru_list.rs:107-118` |
| FR-007 len(pool) | Aligned | `src/lib.rs:151-160` |
| FR-008 clear_pool(pool) | Aligned | `src/lib.rs:162-168`; `lru_list.rs:148-154` |
| FR-009 Result methods → InvalidPool; Option/scalar degrade | **Drifted (low)** → BACKFILLED | `src/lib.rs:60-67,75-83,119-127,135,142-147,153-158,164` — see D1 |
| FR-010 touch/remove idempotent on stale handle, Ok(()); InvalidHandle unused | Aligned | `lru_list.rs:71-73,122-124` (active flag) |
| FR-011 free-list node recycling | Aligned | `lru_list.rs:21,38-45,143` |
| FR-012 batch_touch amortizes lock | Aligned | `src/lib.rs:89-115` (single lock per contiguous same-pool run; relocks on pool change) |

### Non-Functional Requirements

| ID | Status | Location |
|----|--------|----------|
| NFR-001 O(1) single-entry ops | Aligned | index-based DLL, `lru_list.rs` |
| NFR-002 thread-safe, no corruption | Aligned | `RwLock` + per-pool `Mutex`; `concurrent_access` `src/lib.rs:308-336` |
| NFR-003 per-pool locking granularity | Aligned | `Vec<Mutex<Pool>>` behind `RwLock` `src/lib.rs:22-25` |
| NFR-004 component model, provides IEvictionPolicy, ILogger receptacle | Aligned | `define_component!` `src/lib.rs:27-38`; ILogger now actively used (`src/lib.rs:47-49,61-65,76-81,120-126`) — the prior NFR-004 align task (wire logging) is **satisfied** |

### Success Criteria

| ID | Status | Notes |
|----|--------|-------|
| SC-001 lib.rs + lru_list.rs tests pass | Aligned (verified) | 22 passed: 9 in `lib.rs`, 13 in `lru_list.rs` |
| SC-002 4×100 concurrent, no corruption | Aligned (verified) | `concurrent_access` `src/lib.rs:308-336` |
| SC-003 clippy -D warnings + fmt | Aligned (verified) | both clean, re-run this session |
| SC-004 integrates via query_interface!/receptacle in consumers | Aligned (verified) | consumers wire it; see D2 for a stale consumer list in Dependencies |

## Drifts (resolved this run — BACKFILL)

### D1 — FR-009 omits `batch_touch` from the Result-returning set (low)

FR-009 enumerated the `Result`-returning methods as `track, touch, remove`, but
`batch_touch` also returns `Result<(), EvictionPolicyError>` and returns
`InvalidPool` on a non-existent pool (`src/lib.rs:98`, `:109`). Spec text was
incomplete. **Backfilled**: `batch_touch` added to the FR-009 method list.

### D2 — Dependencies/Consumer list stale (low)

Spec `Dependencies` (line ~90) and plan Consumer Graph listed 7 consumers but
omitted `apps/eviction-replay-benchmark`, a real consumer
(`apps/eviction-replay-benchmark/Cargo.toml: eviction-policy-lru.workspace = true`;
uses `EvictionPolicyLruComponent` in `src/main.rs`, `src/sim.rs`,
`tests/replay_hits.rs`). `certus-connector` (repo-root crate) was verified as a
genuine consumer (`certus-connector/Cargo.toml:23`, `src/engine.rs`).
**Backfilled**: `eviction-replay-benchmark` added to spec Dependencies and plan
Consumer Graph. Plan test counts also corrected (8→9 lib, 12→13 lru_list).

## Human Decision (left in report — NOT auto-applied)

### H1 — Interface `track()` documents idempotent re-registration; LRU does not implement it

The interface contract (`components/interfaces/src/ieviction_policy.rs:84-86`)
states: *"Re-registering a key already tracked in `pool` is idempotent: it
refreshes recency and returns the existing handle without creating a new node or
altering lineage."* The LRU implementation unconditionally calls
`lru.push_back(key)` (`src/lib.rs:69`) with **no duplicate-key detection**, so
tracking the same key twice creates two independent nodes (inflating `len`,
producing duplicate eviction candidates, and orphaning the first handle's slot
until evicted).

Spec FR-002 is silent on duplicate-key behavior, so it does not directly
contradict the code — but the interface it conforms to does. This is an
interface-vs-implementation divergence. **Interfaces are out of scope to edit in
this workflow**, and fixing the implementation is a code change. Decision needed:
(a) implement idempotent re-registration in the LRU component to honor the
interface contract, or (b) relax the interface doc to make idempotent
re-registration policy-optional and document LRU's always-append behavior in
FR-002. Recorded as HUMAN_DECISION.

## Coverage gap (align task)

### A1 — FR-012 `batch_touch` has no dedicated test

`batch_touch` is implemented (`src/lib.rs:89-115`) and exercised indirectly, but
there is no unit/integration test asserting its behavior (single-pool amortized
touch, multi-pool relock path, empty-slice early return, invalid-pool error).
SC-001 ("tests pass") is not violated, but coverage is thin for a hot-path
method. Recorded as an ALIGN task (code change — not applied here).

## Recommendations

- D1/D2 backfilled — spec/plan now match implementation reality.
- Address A1 (add `batch_touch` tests) per `align-tasks.md`.
- Resolve H1 (interface idempotency contract vs LRU append behavior) with a human.
