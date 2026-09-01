---
spec_sync_component: dispatch-map
spec_sync_drift_status: clean
spec_sync_synced_at: 2026-09-01T16:30:00Z
spec_sync_git_commit: c410ac44
spec_sync_inputs_sha256: 365aaa3e1934f3923cf1f02acdf592b4afd9e9bc70a85265e361db7e27c8be44
spec_sync_hash_tool: scripts/spec-sync-hash.sh
---
# Dispatch-Map — Spec ↔ Implementation Drift Report

**Generated**: 2026-09-01
**Component**: `components/dispatch-map`
**Branch**: `evolve-dispatcher`

## Summary

| Metric | Count |
|---|---|
| Specs Analyzed | 1 |
| Requirements Checked | 33 (27 FR + 6 SC) |
| Aligned | 30 |
| Drifted | 3 |
| Not Implemented | 0 |
| Unspecced | 0 |

Spec analyzed: `001-dispatch-map` — *Dispatch Map Component* (Status: Complete, Last Synced 2026-08-20).

This sweep re-analyzes the dispatch map after the sharding change on the
`evolve-dispatcher` branch (`c410ac44`): the map was split from a single
`Mutex<Inner>` + `Condvar` into `N_SHARDS=64` independent `Shard`s, each with
its own `Mutex<Inner>` + `Condvar`, with key routing via `key as usize %
N_SHARDS`. This is an internal structural change that does not alter the external
API contract or behavioral semantics — all per-key operations route to their shard
first; multi-key operations (`oldest_keys`) delegate to `IEvictionPolicy` which
is external.

## Detailed Findings — `001-dispatch-map`

### Aligned ✓

All FR-001 through FR-029 and SC-001 through SC-006 items from the previous
report remain aligned, with the same three exceptions below.

### Drifted ⚠️

- **FR-002 / FR-013 / Key Entities (sharded synchronization)** — severity: minor (documentation).
  - Spec: "Reference counts are protected by a `Mutex`/`Condvar` pair for blocking
    semantics" (FR-002). "All `IDispatchMap` methods MUST be thread-safe and
    re-entrant" (FR-013). Key Entities: "Protected by `Mutex`/`Condvar`".
  - Actual: The map is now internally sharded into `N_SHARDS=64` independent
    partitions (`src/state.rs:16`). Each shard has its own `Mutex<Inner>` +
    `Condvar` (`src/state.rs:20-22`). Operations route to `key as usize % N_SHARDS`
    (`src/state.rs:97`), so threads operating on different keys rarely contend on the
    same lock. The blocking `wait_for` semantics are unchanged within each shard.
    `pool_id` remains a single shared `Mutex<Option<PoolId>>`.
  - Location: `src/state.rs:16-98`, `src/lib.rs` (all methods now call
    `self.state.shard_for(key)` before locking).

- **FR-012 — initialize returns error (not panic) if `IEvictionPolicy` unbound** — *moderate* (pre-existing, unchanged).
  - Spec: "returns an error if unbound".
  - Actual: `get_pool_id()` calls `self.eviction_policy.get().unwrap()` which
    panics. Pre-existing; tracked in `.specify/sync/align-tasks.md`.
  - Location: `src/lib.rs:55`.

- **FR-003 / US1-AS3 — null pointer to `create_memory_tier_entry` returns an error** — *moderate* (pre-existing, unchanged).
  - Spec: "a null pointer returns an error; no entry is recorded".
  - Actual: no null-pointer check. Pre-existing; tracked in `.specify/sync/align-tasks.md`.
  - Location: `src/lib.rs:381-424`.

### Not Implemented ✗

None.

## Unspecced Code

None (the `reuse_count` field was specced as FR-029 in the 2026-08-20 sync).

## Recommendations

1. **FR-002 / Key Entities (backfill)**: update the synchronization description to
   reflect the sharded architecture — `N_SHARDS=64` independent `Shard`s each with
   their own `Mutex<Inner>` + `Condvar`; key routing `key % N_SHARDS`.
2. **FR-012 / FR-003 (align)**: the two pre-existing align tasks remain unresolved.
   Not addressed in this sync (code-side changes are outside the spec-sync scope).
