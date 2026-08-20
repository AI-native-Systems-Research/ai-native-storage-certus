# Spec-Sync Phase B — Proposals: memory-tier

**Generated**: 2026-08-20
**Component**: memory-tier · **Spec**: `001-memory-tier`
**Policy**: `.specify/sync/PHASE_B_POLICY.md`
**Decision (flagship "backfill to reality")**: the single `RwLock<Pool>` implementation is the
intended, working design. The 16-way sharded + Creusot-verified design described in earlier spec
revisions was **never built**. The spec is backfilled to match the code; no `.rs` source is changed.

## Summary

| Direction | Count |
|-----------|-------|
| BACKFILL (applied to spec.md) | 9 |
| ALIGN (task, no code change) | 0 |
| BACKFILL-UNSPECCED | 0 |
| RESOLVED (already fixed on main) | 0 |
| HUMAN_DECISION | 1 |

## Proposals

### FR-005 — BACKFILL
- **Rationale**: Spec mandated 16 independent shards; `src/lib.rs:76-85,313-316` holds one `FreeList` + one `HashMap` behind a single `RwLock<Pool>`. No `Shard`, `NUM_SHARDS`, or `shard_for_key` exists.
- **Before**: `Pool is divided into 16 independent shards`
- **After**: `Pool state (allocator + slot map) is held as a single, unsharded structure behind one RwLock<Pool> (no shards)`

### FR-006 — BACKFILL
- **Rationale**: No key-modulo-16 shard selection exists. Rewrote to describe the real single reader-writer lock concurrency model; dropped stale `Creusot P4, P5` claim.
- **Before**: `Shard selection uses key modulo 16` (Verified: Creusot P4, P5)
- **After**: `Concurrency uses one reader-writer lock: reads take a shared read lock; mutations take an exclusive write lock` (Verified: Implementation)

### FR-007 — BACKFILL
- **Rationale**: One allocator + slot map under a single `RwLock` (`src/lib.rs:76-79`), not per-shard Mutex structures.
- **Before**: `Each shard has its own Mutex-protected allocator and slot map`
- **After**: `A single first-fit FreeList allocator and a single HashMap<CacheKey, Slot> slot map serve the whole pool`

### FR-013 — BACKFILL
- **Rationale**: `evict_next` delegates to `ep.identify_next_to_evict(pool_id)`; no `evict_counter`/round-robin (`src/lib.rs:434-466`). Dropped stale `Creusot P10`.
- **Before**: `evict_next() cycles through shards via atomic counter (round-robin)` (Verified: Creusot P10)
- **After**: `evict_next() delegates victim selection to the eviction policy (identify_next_to_evict(pool_id)), then removes that slot and frees its allocation; there is no shard round-robin counter` (Verified: Implementation)

### FR-014 — BACKFILL
- **Rationale**: `src/lib.rs:468-470` binds `_key` and calls `evict_next()`; a pure alias. With a single global pool there is no shard to target, so this is correct/intended. Dropped stale `Creusot P4, P5`.
- **Before**: `evict_next_for_key() evicts from the same shard as the target key` (Verified: Creusot P4, P5)
- **After**: `evict_next_for_key(key) is an alias for evict_next(); the key argument is ignored because the pool is not sharded` (Verified: Implementation)

### FR-021 — BACKFILL
- **Rationale**: `oldest_keys` makes a single `ep.get_eviction_candidates(pool_id, n)` call (`src/lib.rs:424-432`); no `(n/NUM_SHARDS).max(1)` sampling.
- **Before**: `oldest_keys(n) peeks at N oldest keys across shards`
- **After**: `oldest_keys(n) returns up to N oldest keys via a single IEvictionPolicy::get_eviction_candidates(pool_id, n) call (no per-shard sampling)`

### NFR-002 — BACKFILL
- **Rationale**: Listed under `not_implemented` ("16-way sharded ... NFR-002"). Rewrote to the actual single-`RwLock` model.
- **Before**: `Per-shard locking minimizes contention (16-way parallelism)`
- **After**: `A single RwLock<Pool> serializes mutations while allowing concurrent readers; data-path touches are applied outside the pool lock via the eviction policy's own synchronization`

### IMemoryTier-doc-verified-claims / SC-8 — BACKFILL
- **Rationale**: No proof artifacts exist under `components/memory-tier/verif/` and no shards exist. Removed Success Criterion #8 and every `Creusot P#` entry in the FR Verified column (FR-006/008/009/010/013/014/015/023). The interface-doc `P4/P5/P10 "Verified"` overclaiming was **already removed** from `components/interfaces/src/imemory_tier.rs`; policy forbids re-adding it. A residual "same shard as key" phrase remains in that file's `evict_next_for_key` doc comment — **out of this component's edit scope** (noted, not edited).
- **Before**: SC-8 `"10 formal properties verified with Creusot (21 verification conditions)"`; FR Verified entries `Creusot P1/P2/P3/P4/P5/P8/P9/P10`
- **After**: SC-8 removed; Creusot annotations replaced with `Unit test` or `Implementation`

### not_implemented: 16-way sharded allocator (FR-005/006/007, NFR-002) — BACKFILL
- Never-built design; resolved by the FR-005/006/007 + NFR-002 backfills above. Spec no longer mandates the unbuilt structure.

### not_implemented: Round-robin eviction counter (FR-013) — BACKFILL
- Never-built; resolved by the FR-013 backfill. Spec no longer mandates an `evict_counter`.

### NFR-008 — HUMAN_DECISION
- **Rationale**: Three-way version conflict with no authoritative value — `Cargo.toml` = `0.1.0`, `define_component!` macro = `0.3.0` (`src/lib.rs:140`), spec = `0.2.0`. Reconciling requires editing `Cargo.toml` and `src/lib.rs` (out of scope) and choosing a single real version; none of the three is obviously correct.
- **Disposition**: spec text **left unchanged**; documented in `align-tasks.md` for a maintainer to reconcile.
