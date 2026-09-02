# Spec-Sync Proposals — memory-tier

**Generated**: 2026-09-02
**Component**: memory-tier · **Spec**: `001-memory-tier`

`spec.md` was already backfilled to the single-`RwLock<Pool>` reality (2026-08-20)
and is aligned with `src/`. This pass extends that backfill to the two supporting
spec artifacts that were left stale, and records the still-open version conflict.

## Summary

| Direction | Count | approved |
|-----------|-------|----------|
| BACKFILL (applied to plan.md / tasks.md) | 2 | true |
| ALIGN (task, no code change) | 0 | — |
| HUMAN_DECISION | 3 | n/a |

## Proposals

### plan.md architecture + Creusot sections — BACKFILL (approved)
- **Rationale**: `plan.md` still described a 16-way sharded pool and a Creusot
  P1–P10 verification section (21 VCs). No sharding or proofs exist in `src/`
  (`grep shard|NUM_SHARDS|creusot` → nothing; no `verif/`). The 2026-08-20 Phase B
  decision resolved the sharding fate by backfilling `spec.md` to the single-pool
  design; `plan.md` simply wasn't brought in line. This is a confident BACKFILL
  consistent with that resolved decision.
- **Change**: Rewrote Summary, Memory Layout, Pointer Arithmetic, Concurrency
  Model, Key Design Decisions #1/#6 to the single-`RwLock<Pool>` design; replaced
  the "Formal Verification (Creusot)" section with an explicit "None" note;
  reworded shard-related Future Considerations; updated the trailing Spec-Sync
  Notes.

### tasks.md stale Creusot/shard tasks — BACKFILL (approved)
- **Rationale**: `tasks.md` listed "Confirm formal verification properties
  (P1-P10)", "Verify Creusot verification conditions still discharge", "Add test
  for evict_next_for_key targeting correct shard", a "shard layout" diagram task,
  and a "configurable shard count" backlog item — all referencing the never-built
  sharded/verified design.
- **Change**: Removed the Creusot and shard-targeting tasks; reworded the layout
  diagram task and the backlog item to the single-pool reality; clarified the
  README `lru.rs` task.

### NFR-008 component version — HUMAN_DECISION (not approved for auto-apply)
- **Rationale**: Three-way conflict — `Cargo.toml` = `0.1.0`, `define_component!`
  = `0.3.0` (`src/lib.rs:140`), `spec.md` NFR-008 = `0.2.0`. No authoritative
  value; reconciling requires editing `Cargo.toml` + `src/lib.rs` (out of scope).
- **Disposition**: spec text left unchanged; recorded in `align-tasks.md`.

### IMemoryTier `evict_next_for_key` doc comment — HUMAN_DECISION (out of scope)
- **Rationale**: `components/interfaces/src/imemory_tier.rs:87-91` still says
  "same shard as `key`" / "target shard is empty". Pool is unsharded; `key` is
  ignored.
- **Disposition**: `components/interfaces/**` is outside this component's edit
  scope. Left for a cross-cutting interface fix.

### README source layout (`lru.rs`) — HUMAN_DECISION (out of spec-sync scope)
- **Rationale**: `README.md:23-30` lists a nonexistent `src/lru.rs`.
- **Disposition**: Doc-only, not under `specs/**`; not edited. Tracked in
  `tasks.md` Documentation.
