---
spec_sync_component: eviction-policy-lru
spec_sync_drift_status: clean
spec_sync_synced_at: 2026-09-04T17:01:54Z
spec_sync_git_commit: 0aa01097
spec_sync_inputs_sha256: 46def3c25a0b610ec89dc68887bf3dee83e2ac98aaeb23353381a1f633f86884
spec_sync_hash_tool: scripts/spec-sync-hash.sh
---
# Drift Report: eviction-policy-lru

**This sweep (2026-09-04)** independently re-verified spec
`001-lru-eviction-policy` (FR-001..FR-012, NFR-001..004, SC-001..004) against
`src/{lib,lru_list}.rs` and the shared interface
(`components/interfaces/src/ieviction_policy.rs`). The prior report
(2026-08-07) claimed "20 aligned, 0 drift, clean" but was **unstamped** and did
not examine the `track()` re-registration contract. A fresh read found **two
documentation drifts** (both resolved by BACKFILL this sweep) and one
cross-component contract question that was escalated and resolved by an explicit
product decision (**document LRU, defer the interface** — see below). No code
bug was found; no code was changed.

**Why `drift_status: clean`:** after the two BACKFILLs, the component's own spec
now accurately describes the implementation, and `src/` is unchanged and green
(22 unit tests pass, `clippy -D warnings` clean, `fmt --check` clean — verified
locally this sweep). The only residual is a wording choice in the *shared*
`IEvictionPolicy` interface doc (idempotency clause worded as universal but
honored only by lineage policies); that is a coordinated interfaces-crate change,
deliberately deferred (editing `components/interfaces/` invalidates every stamped
component's folded hash), and is recorded as a non-blocking follow-up rather than
left as unresolved component drift.

## Summary

| Metric | Count |
|---|---|
| Specs Analyzed | 1 (`001-lru-eviction-policy`) |
| Requirements Checked | 20 (FR-001..012 + NFR-001..004 + SC-001..004) |
| Aligned | 20 |
| Drift → doc fix applied (BACKFILL) | 2 (D1 FR-002 re-registration; D2 `EvictionKey`→`CacheKey`) |
| Drift → code fix (ALIGN) | 0 |
| Cross-component (deferred by decision) | 1 (interface idempotency wording) |
| Not Implemented | 0 |
| Unspecced | 0 |

## Resolved this sweep — doc fixes (BACKFILL; doc lag / doc error against correct code)

**D1 — `track()` idempotent-re-registration contract (`spec.md` FR-002; interface
doc `ieviction_policy.rs:82-87`).** The shared `IEvictionPolicy::track` doc states
re-registration "is idempotent: it refreshes recency and returns the existing
handle without creating a new node." The LRU implementation is **NOT** idempotent:
`track` unconditionally `push_back`s a new node and returns a new handle
(`src/lib.rs:69`). Investigation established this is **not a live bug**:
- The idempotency clause was added in interfaces commit `1da4e777` (the
  session-lineage policy commit), whose message states *"eviction-policy-lru
  ignores the new param (behavior preserved)"* — LRU's non-idempotent behavior was
  intentional, not an oversight.
- Only the sibling lineage policy implements it: `eviction-policy-session-lists`
  `register()` dedupes via a `by_key` map (`session_list.rs:90-119`).
- **No production caller re-tracks a live key.** All consumers guard first:
  `dispatch-map` (`src/lib.rs:395-399`, `576-579`, `contains_key`→`AlreadyExists`)
  and `memory-tier` (`src/lib.rs:356-366`, same guard); the benchmark
  (`apps/eviction-replay-benchmark/src/sim.rs`) removes a key before re-tracking.
- The LRU component's *own* spec never promised idempotency.

**Resolution (BACKFILL, per product decision "document LRU, defer interface"):**
reworded FR-002 to explicitly state re-registration is **not** idempotent in this
policy (new node + new handle on re-`track`), that this deliberately diverges from
the interface's general clause (written for lineage-aware policies), and that
callers MUST NOT re-`track` a live key (consumers enforce this upstream). This is
a BACKFILL, not a masked bug: the behavior is intentional, exercised by no caller,
and never claimed by this component's spec.

**D2 — non-existent `EvictionKey` entity (`spec.md` Key Entities; `plan.md:16`).**
The Key Entities list named "**EvictionKey** (`u64`) … same underlying type as
`CacheKey`", but there is **no** `EvictionKey` type or alias anywhere in the code
(grep confirms: mentioned only in the spec/plan). The tracked key is
`interfaces::CacheKey` (`idispatch_map::CacheKey`, `u64`) throughout `src/` and the
interface. **Resolution (BACKFILL):** renamed the entity to `CacheKey` with a note
that no distinct `EvictionKey` exists; corrected the identical stale reference in
`plan.md:16`.

## Cross-component follow-up (deferred by explicit decision — non-blocking)

**Interface idempotency wording over-generalization.** The
`IEvictionPolicy::track` doc (`components/interfaces/src/ieviction_policy.rs:82-87`)
states re-registration idempotency as an unqualified contract for all implementors,
yet only lineage-aware policies honor it (LRU deliberately does not — see D1). The
clean root-cause fix is to scope the interface wording to "policy-defined" (idempotent
for lineage policies; non-idempotent for LRU). This was **deliberately deferred**
(product decision this sweep): editing `components/interfaces/` invalidates the
folded input hash of **every** stamped component, so it belongs in a coordinated
interfaces pass, not a single-component sync. Recorded here so the divergence is
tracked rather than silently stamped away. Until then, the LRU-side divergence is
fully documented in this component's FR-002 (D1).

## Known limitation (pre-existing; not spec↔code drift; non-blocking)

**Stale-handle ABA after free-list recycling.** FR-010 guarantees `touch`/`remove`
on an *already-removed* handle are no-ops (via the `active` flag,
`lru_list.rs:71-73,122-124`). But FR-011's free-list recycling
(`lru_list.rs:38-45,143`) reuses a removed node's slot index for a later `track`.
A handle retained across that removal+recycle would then reference the slot's *new*
occupant (a live node), so `touch`/`remove` would silently affect the wrong entry —
the classic index-handle ABA hazard. The spec does not promise ABA safety: FR-010
scopes its guarantee to an "already-removed handle", and the `InvalidHandle`
variant is explicitly "reserved for future stricter validation" (FR-010). No caller
retains handles across a remove of the same key today. Fixing it properly needs a
generation counter in the handle — a code + interface change, out of scope for a
doc sync. Recorded as a latent hazard, not stamped as drift.

## Verification

- **Local (authoritative — this is a pure-Rust default workspace member):**
  `cargo test -p eviction-policy-lru` → 22 passed / 0 failed (SC-001, SC-002).
  `cargo clippy -p eviction-policy-lru --all-targets -- -D warnings` → clean (SC-003).
  `cargo fmt -p eviction-policy-lru --check` → clean (SC-003). All run this sweep.
- **No `src/` change** this sweep — the two resolutions are spec-doc BACKFILLs only,
  so the green build state is unaffected and the input hash is unchanged.

## Gate & tooling note (important)

This component stores its specs under `.specify/specs/`, **not** a top-level
`components/eviction-policy-lru/specs/`. Two consequences:
1. The CI Spec-Sync Gate discovers targets via `find components -maxdepth 2 -type d
   -name specs` — which does **not** match `.specify/specs`, so this component is
   **not gated by CI** regardless of this stamp (same situation as
   `disk-partition-manager`).
2. `scripts/spec-sync-hash.sh` hashes `<dir>/src` + `<dir>/specs` (+ interfaces);
   with no top-level `specs/`, the digest covers `src/` + interfaces **only** and
   does **not** include this spec markdown. Editing the spec does not change the
   hash. The stamp therefore certifies the `src/`+interfaces inputs; the spec
   accuracy rests on this human-readable report.

To make this component genuinely gated and hash-covered, its `.specify/specs/`
tree would need to move to a top-level `specs/` dir (or the tooling taught about
the `.specify/specs/` layout) — a structural change out of scope for this sync.

## Aligned ✓

| ID | Evidence |
|----|----------|
| FR-001 create_pool sequential from 0 | `src/lib.rs:41-51` |
| FR-002 track → MRU, returns handle, ignores semantics; non-idempotent (documented) | `src/lib.rs:53-71`, `:69` |
| FR-003 touch O(1) → MRU | `src/lib.rs:73-87`; `lru_list.rs:70-96` |
| FR-004 remove O(1) unlink | `src/lib.rs:117-131`; `lru_list.rs:121-145` |
| FR-005 identify_next_to_evict removes+returns LRU or None | `src/lib.rs:133-138`; `lru_list.rs:99-104` |
| FR-006 get_eviction_candidates(n) no removal, O(n) | `src/lib.rs:140-149`; `lru_list.rs:107-118` |
| FR-007 len(pool) | `src/lib.rs:151-160` |
| FR-008 clear_pool(pool) | `src/lib.rs:162-168`; `lru_list.rs:148-154` |
| FR-009 Result methods → InvalidPool; Option/scalar degrade | `src/lib.rs:60-67,75-83,119-127,135,142-147,153-158,164` |
| FR-010 touch/remove idempotent on removed handle, Ok(()); InvalidHandle unused | `lru_list.rs:71-73,122-124` |
| FR-011 free-list node recycling | `lru_list.rs:21,38-45,143` |
| FR-012 batch_touch single lock per pool-run | `src/lib.rs:89-115` |
| NFR-001 O(1) single-entry ops | index-based DLL, `lru_list.rs` |
| NFR-002 thread-safe, no corruption | `RwLock`+per-pool `Mutex`; `concurrent_access` `src/lib.rs:308-336` |
| NFR-003 per-pool locking granularity | `Vec<Mutex<Pool>>` behind `RwLock` `src/lib.rs:22-25` |
| NFR-004 component model, provides IEvictionPolicy, ILogger receptacle | `define_component!` `src/lib.rs:27-38` |
| SC-001 lib.rs + lru_list.rs tests pass | 22 tests pass (this sweep) |
| SC-002 4×100 concurrent, no corruption | `concurrent_access` `src/lib.rs:308-336` |
| SC-003 clippy -D warnings + fmt | clean (this sweep) |
| SC-004 integrates via query_interface!/receptacle in consumers | consumer crates (spec-declared; not re-verified this sweep) |

## Unspecced Features

None. `peek_front_n` and `Node.active` are internal helpers already described in
the spec's Implementation Notes; no public surface beyond `IEvictionPolicy`.
