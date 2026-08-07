# Align Tasks — dispatcher

Generated: 2026-08-07 (branch `sync/spec-drift-sweep-20260807`)
Source: `drift-report.{json,md}` (generated 2026-08-07)

This sweep produced **no spec→code behavior changes**. All drifted requirements
were resolved by BACKFILL (spec updated to match working, tested code). The one
ALIGN item is a documentation-accuracy correction, already applied on the branch.

---

## Task 1 (DONE) — Soften phantom Creusot verification claims  [doc-only, MODERATE]

**Problem**: `components/interfaces/src/idispatcher.rs` advertised a Creusot
proof tree at `components/dispatcher/verif/` — "10 properties, 24 verification
conditions discharged by SMT solvers" — and tagged 16 methods with
`# Verified: Pn` headings. No `verif/` directory and no Creusot proofs exist
anywhere in the crate (confirmed by `ls` and a repo-wide `creusot` grep). The
interface docs asserted formal verification that was never present.

**Change (applied on branch)**:
- Block comment reframed to "Design Invariants (informal — NOT machine-checked)",
  explicitly recording that the prior Creusot/SMT claim was false and removed.
- P9/P10 list entries completed (previously cited only in method docs while the
  block listed P1–P8 yet claimed "10 properties").
- All 16 `/// # Verified: ` headings changed to
  `/// # Design invariant (informal, not machine-checked): `.

**Files Modified**: `components/interfaces/src/idispatcher.rs`.

### Acceptance Criteria
- [x] `cargo build -p interfaces` — clean.
- [ ] **REVIEW**: confirm the softened wording is acceptable, or decide to invest
      in restoring genuine Creusot proofs (then restore the verified-status
      wording and point it at the actual proof artifacts).

---

## Informational — resolved by BACKFILL (no code change)

Per the "Backfill all to spec" decision, the following were resolved by editing
`specs/001-dispatcher-cache-interface/spec.md` (see `apply-report.md`):

- **FR-001** — method inventory expanded to the full shipped `IDispatcher` surface.
- **FR-039** — `batch_lookup` signature corrected to `&[(CacheKey, Vec<IpcHandle>)]`
  (multi-region scatter).
- **FR-042** — `create_eviction_channel(capacity: usize)`.
- **FR-033** — added `metadata_partition_size`, `extended_metadata_partition_size`,
  `backfill_delay_ms` (p2p-only) config fields.
- **FR-056 (new)** — GPU-staged memory-lifecycle primitives (reserve/copy/
  complete/release/pin/unpin) + `flush_to_ssd`/`read_write_stats`.
