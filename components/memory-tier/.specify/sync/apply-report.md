# Sync Apply Report — memory-tier

**Component**: memory-tier · **Spec**: `001-memory-tier`
**Applied**: 2026-09-02
**Source**: `.specify/sync/drift-report.json` (1 spec-numbered drift, 2 supporting-artifact drifts, 0 not_implemented, 0 unspecced)

`spec.md` was already aligned to the single-`RwLock<Pool>` implementation
(2026-08-20 Phase B). This pass re-verified every requirement and extended the
backfill to the supporting spec artifacts (`plan.md`, `tasks.md`) that still
described the never-built 16-way sharding + Creusot P1–P10 material.

## Summary

| Direction | Count |
|-----------|-------|
| BACKFILL applied | 2 |
| ALIGN tasks generated | 0 |
| Unspecced backfilled | 0 |
| HUMAN_DECISION | 3 |

## Specs Updated (BACKFILL)

| File | Change |
|------|--------|
| `plan.md` | Rewrote Summary, Memory Layout, Pointer Arithmetic, Concurrency Model, Key Design Decisions #1/#6 to the single-`RwLock<Pool>` design; replaced the "Formal Verification (Creusot)" P1–P10 section with an explicit "None" note; reworded shard-related Future Considerations; refreshed trailing Spec-Sync Notes. |
| `tasks.md` | Removed "Confirm formal verification properties (P1-P10)", "Verify Creusot verification conditions", and "test for evict_next_for_key targeting correct shard"; reworded the "shard layout" diagram task and "configurable shard count" backlog item to the single-pool reality; clarified the README `lru.rs` task. |

`spec.md` itself was **not** modified this pass (already aligned).

## Align Tasks Generated

None. `spec.md` is aligned with `src/`; no code bug exists. See
`.specify/sync/align-tasks.md`.

## Human Decision

| Item | Detail |
|------|--------|
| NFR-008 version | `Cargo.toml` 0.1.0 / `define_component!` 0.3.0 / `spec.md` 0.2.0 disagree; no authoritative value. Requires editing `Cargo.toml` + `src/lib.rs` (out of scope). Keeps drift_status = `drift`. |
| `evict_next_for_key` doc | `components/interfaces/src/imemory_tier.rs:87-91` still says "same shard as key"; interfaces crate is out of scope. |
| README source layout | `README.md:23-30` lists nonexistent `src/lru.rs`; doc-only, not under `specs/**`. |

## Backup

Originals backed up before editing:
- `.specify/sync/backups/2026-09-02T21:40:18Z/specs/001-memory-tier/plan.md`
- `.specify/sync/backups/2026-09-02T21:40:18Z/specs/001-memory-tier/tasks.md`
- `.specify/sync/backups/2026-09-02T21:40:18Z/drift-report.md.prev` (stale prior report)

## Files Touched

- `components/memory-tier/specs/001-memory-tier/plan.md` (backfilled)
- `components/memory-tier/specs/001-memory-tier/tasks.md` (backfilled)
- `components/memory-tier/.specify/sync/drift-report.md` (regenerated + stamped) · `drift-report.json`
- `components/memory-tier/.specify/sync/proposals.md` · `proposals.json`
- `components/memory-tier/.specify/sync/align-tasks.md`
- `components/memory-tier/.specify/sync/apply-report.md` (this file) · `apply-report.json`
- `components/memory-tier/.specify/sync/backups/2026-09-02T21:40:18Z/**`

## Not Modified (out of scope / policy)

`src/lib.rs`, `src/allocator.rs`, `Cargo.toml`, `spec.md` (already aligned),
`components/interfaces/src/imemory_tier.rs`, `README.md`.
