---
spec_sync_component: dispatch-map
spec_sync_drift_status: clean
spec_sync_synced_at: 2026-09-09T21:33:18Z
spec_sync_git_commit: 5bb71702
spec_sync_inputs_sha256: 7f74b8e980be9afe665117e3ba9100092073f8f7960e78ad9620f0656cb810a7
spec_sync_hash_tool: scripts/spec-sync-hash.sh
---
# Dispatch-Map — Spec ↔ Implementation Drift Report

**Generated**: 2026-09-02 (last analysis); **Restamped**: 2026-09-09
**Component**: `components/dispatch-map`
**Branch**: `fix-dispatcher-store-backpressure` (restamp); `evolve-dispatcher-dw` (analysis)
**Mode**: Read-only drift analysis, then BACKFILL apply to `spec.md` (code authoritative).

> **Refresh 2026-09-09 (restamp only — no drift, no spec change):** the stamp was
> refreshed to input hash
> `7f74b8e980be9afe665117e3ba9100092073f8f7960e78ad9620f0656cb810a7` at commit
> `5bb71702`. The previous stamp (`2dbb7666…`) went stale for two reasons, neither
> of which is dispatch-map drift: (1) `components/dispatch-map/specs/001-dispatch-map/spec.md`
> was already backfilled to the global-lock design in commit `6a3d801c` (the sweep
> documented below), and (2) `scripts/spec-sync-hash.sh` folds **all** of
> `components/interfaces/` into every component's hash, so the dispatcher-only
> `idispatcher.rs` store-backpressure change (`39ffee9b`/`c400ced8`, new
> `DispatcherConfig::store_backpressure_ms` + two `TierEventStats` counters)
> re-hashed dispatch-map even though it references none of those types. Re-verified
> this sweep: `src/state.rs` still uses a single global `Mutex<Inner>` + `Condvar`
> (matches FR-002/FR-013); no `IDispatchMap` method or type changed. **Drift status
> remains `clean`; no `spec.md` edit was needed.**

## Summary

| Metric | Count |
|---|---|
| Specs Analyzed | 1 |
| Requirements Checked | 33 (27 FR + 6 SC) |
| Drift findings this sweep | 1 (backfilled) |
| Pre-existing aligns (unchanged) | 2 |
| Not Implemented | 0 |
| Unspecced | 0 |

Spec analyzed: `001-dispatch-map` — *Dispatch Map Component*.

This sweep re-analyzes the dispatch map after the **sharding revert** (commit
`8f494f8d`) on this branch. The prior sync (`2026-09-01`, commit `c410ac44`)
had backfilled the spec to describe an `N_SHARDS=64` sharded synchronization
scheme introduced by `20fc8f14`. That scheme was subsequently **reverted in
code**: `src/state.rs` and `src/lib.rs` are back to a single global
`Mutex<Inner>` + `Condvar` (the origin/unstable design). The spec therefore
now *over-describes* sharding that no longer exists — one drift finding,
resolved by backfilling the spec back to the global-lock description.

## Detailed Findings — `001-dispatch-map`

### Drifted → resolved by BACKFILL ⚠️

- **FR-002 / FR-013 / Key Entities (synchronization model)** — severity: moderate (documentation).
  - Spec (as of the 2026-09-01 sync) said: the map is internally sharded into
    `N_SHARDS=64` independent partitions, each with its own `Mutex<Inner>` +
    `Condvar`, with routing `key as usize % N_SHARDS`.
  - Actual: sharding was reverted by `8f494f8d`. `src/state.rs` defines a single
    `DispatchMapState { inner: Mutex<Inner>, condvar: Condvar, pool_id:
    Mutex<Option<PoolId>> }` where `Inner` holds one `HashMap<CacheKey,
    DispatchEntry>` (`src/state.rs:11-36`). Every `IDispatchMap` method acquires
    this one global lock; blocking waits use the shared `Condvar` via `wait_for`
    (`src/state.rs:52-83`). There is **no** `Shard` type, no `shard_for`, and no
    `N_SHARDS` in the source (`grep -r shard src/` → empty).
  - Rationale for code-authoritative direction: the revert is a shipped bug fix.
    Sharding measured within noise (+2.2%) but introduced two fatal defects —
    (1) cross-shard eviction blindness (PoolFull retry evicted from the wrong
    shard → `AllocationFailed` → vLLM crash) and (2) a Check→Pin ordering race
    that per-shard locks unmasked (the global lock's contention had been masking
    it). Aligning code→spec would reintroduce both crashes.
  - Backfill: FR-002, FR-013, and the Key Entities "Dispatch Entry" bullet
    rewritten to describe the single global `Mutex<Inner>` + `Condvar`; the
    `Last Synced` header note records the revert and why re-sharding is unsafe.
    FR-013 keeps a one-line parenthetical noting the trial-and-revert so the
    history is not lost.

### Pre-existing code-side aligns (unchanged, not addressed by this sweep)

- **FR-012 — initialize/get_pool_id panics if `IEvictionPolicy` unbound** — *moderate*.
  - Spec: "returns an error if unbound". Actual: `self.eviction_policy.get().unwrap()`
    panics. Tracked in `.specify/sync/align-tasks.md`. Code-side change, out of
    this spec-sync's (documentation-only) scope.
- **FR-003 / US1-AS3 — null pointer to `create_memory_tier_entry`** — *moderate*.
  - Spec: "a null pointer returns an error; no entry is recorded". Actual: no
    null-pointer check. Tracked in `.specify/sync/align-tasks.md`.

Both remain `drift`-worthy at the code level but are long-standing, orthogonal
to this sweep, and explicitly deferred (not silently marked clean): the sweep's
`clean` status reflects that no *documentation* drift remains after backfill.
The two aligns are behavior changes owned by `align-tasks.md`.

### Not Implemented ✗

None.

## Unspecced Code

None.

## Recommendations

1. Commit this `drift-report.md` (with the freshness stamp above) together with
   the `spec.md` backfill so the CI Spec-Sync Gate sees a fresh, matching report.
2. Resolve the two pre-existing aligns (FR-012 panic, FR-003 null-pointer check)
   in a separate code change; they are not documentation drift and are out of
   scope here.
