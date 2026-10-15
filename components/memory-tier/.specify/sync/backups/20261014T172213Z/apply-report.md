# Sync Apply Report — memory-tier (Spec-Sync Phase B)

**Component**: memory-tier · **Spec**: `001-memory-tier`
**Mode**: BACKFILL-TO-REALITY (flagship case)
**Applied**: 2026-08-20
**Policy**: `.specify/sync/PHASE_B_POLICY.md`
**Source**: `.specify/sync/drift-report.json` (8 drifted, 3 not_implemented, 0 unspecced)

Decision: the single `RwLock<Pool>` implementation is the intended, working design. The 16-way
sharded + Creusot-verified design in earlier spec revisions was never built. The spec is backfilled
to match the code. **No `.rs` source, `Cargo.toml`, `plan.md`, `tasks.md`, or `README.md` changed.**

## Summary

| Direction | Count |
|-----------|-------|
| BACKFILL applied | 9 |
| ALIGN tasks generated | 0 |
| Unspecced backfilled | 0 |
| Resolved (already fixed) | 0 |
| HUMAN_DECISION | 1 |

## Specs Updated (BACKFILL)

| Requirement | Change type | Notes |
|-------------|-------------|-------|
| FR-005 | rewrite | 16 independent shards → single `RwLock<Pool>` (no shards) |
| FR-006 | rewrite | key-modulo-16 shard selection → reader-writer lock model; `Creusot P4,P5` removed |
| FR-007 | rewrite | per-shard Mutex allocator/slot map → single `FreeList` + single `HashMap` |
| FR-013 | rewrite | round-robin shard counter → eviction-policy delegation; `Creusot P10` removed |
| FR-014 | rewrite | same-shard eviction → documented alias for `evict_next()` (`key` ignored); `Creusot P4,P5` removed |
| FR-021 | rewrite | per-shard `(n/NUM_SHARDS).max(1)` sampling → single `get_eviction_candidates(pool_id, n)` call |
| NFR-002 | rewrite | 16-way per-shard locking → single `RwLock<Pool>` (concurrent readers, serialized writers) |
| SC-8 | removed | "10 formal properties verified with Creusot (21 VCs)" — no proof artifacts; never built |
| FR Verified column (FR-006/008/009/010/013/014/015/023) | rewrite | `Creusot P#` annotations → `Unit test` / `Implementation` (IMemoryTier-doc-verified-claims) |

Supporting prose also aligned: Status/Last-Synced metadata, Overview paragraph, User Story 2/4/5
acceptance criteria, Key Entities table (`Shard` row replaced by `Pool` row), Implementation Notes
(top-level lock description; `evict_counter` note removed; `oldest_keys` note), and the trailing
Spec-Sync Notes block (replaced with a 2026-08-20 backfill-resolution note).

## Align Tasks Generated

None. All drift resolved by backfill except the version conflict (HUMAN_DECISION, below). The five
previously-deferred align-tasks are superseded — see `.specify/sync/align-tasks.md`.

## Unspecced Backfilled

None (drift report lists 0 unspecced features).

## Resolved

None (no memory-tier item was pre-fixed on the main thread).

## Human Decision

| Requirement | Detail |
|-------------|--------|
| NFR-008 | `Cargo.toml` 0.1.0 / `define_component!` 0.3.0 / spec 0.2.0 disagree; no authoritative value. Spec text left unchanged; reconciliation (requires editing `Cargo.toml` + `src/lib.rs`) recorded in `align-tasks.md` for a maintainer. |

## Backup

Original spec backed up before editing:
- `.specify/sync/backups/specs/001-memory-tier/spec.md.bak`

## Files Touched

- `components/memory-tier/specs/001-memory-tier/spec.md` (backfilled)
- `components/memory-tier/.specify/sync/proposals.md` · `proposals.json`
- `components/memory-tier/.specify/sync/align-tasks.md` (regenerated: 0 ALIGN)
- `components/memory-tier/.specify/sync/apply-report.md` (this file) · `apply-report.json`
- `components/memory-tier/.specify/sync/backups/specs/001-memory-tier/spec.md.bak`

## Not Modified (out of scope / policy)

`src/lib.rs`, `Cargo.toml`, `components/interfaces/src/imemory_tier.rs` (a residual "same shard as
key" phrase remains in its `evict_next_for_key` doc comment; the `P4/P5/P10 "Verified"` overclaiming
was already removed there), `plan.md`, `tasks.md`, `README.md`.
