# Spec-Sync Phase B Apply Report — spdk-env

**Mode**: Phase B resolution (BACKFILL + not_implemented handling)
**Applied**: 2026-08-20
**Source**: `components/spdk-env/.specify/sync/drift-report.{json,md}`
**Policy**: `.specify/sync/PHASE_B_POLICY.md` (no special per-component note for spdk-env)
**Backups**: `components/spdk-env/.specify/sync/backups/specs/002-spdk-env-vfio-init/`
(`spec.md.bak`, `tasks.md.bak` — pre-edit copies of every spec Markdown file touched)

## Scope

Only Markdown under `components/spdk-env/specs/**` and
`components/spdk-env/.specify/sync/**` was edited. No `.rs` source was touched;
no cargo was run.

## Drift-report items and their resolution

| Item | Type in report | Direction | Outcome |
|------|----------------|-----------|---------|
| `stale-crate-paths` | drifted (minor) | BACKFILL | Applied to tasks.md + spec.md |
| `FR-015` | not_implemented | LEAVE + NOTE | No edit; noted (already self-flagged in spec) |

## Specs Updated (BACKFILL)

| File | Requirement | Change type | Change |
|------|-------------|-------------|--------|
| `specs/002-spdk-env-vfio-init/tasks.md` | stale-crate-paths | Path correction | T001, T003, T004, T005, T006: `components/spdk-sys/...` → `lib/spdk-sys/...`; added a dated backfill note under the Organization line. `components/spdk-env` retained in T002/T003 (that crate did not move). |
| `specs/002-spdk-env-vfio-init/spec.md` | stale-crate-paths | Path clarification | Line 6 (historical Input): preserved the verbatim `../component-framework` quote and appended a bracketed note that the crate moved to `../../lib/component-framework` (name unchanged, still resolves). |

Not edited (intentionally): `spec.md:99` (FR-001) and `spec.md:188` (Assumptions)
reference the crate **name** "component-framework", which is unchanged and still
resolves as a workspace dependency — not a stale filesystem path, so no edit.

## Align Tasks Generated

None. The single drift was spec-lag (BACKFILL); FR-015 is deferred future work,
not a behavioral bug. See `align-tasks.md` (2026-08-20 section) for the rationale;
prior-run tasks are retained there for history.

## Unspecced Backfilled

None (drift report lists 0 unspecced features).

## Resolved (already fixed on main thread)

None applicable this run.

## not_implemented Handled

| Requirement | Handling | Notes |
|-------------|----------|-------|
| FR-015 (skip in-use devices, warn per skipped device, return only probed devices) | LEAVE + NOTE | Genuinely intended future behavior from Clarifications (Session 2026-04-07); spec already annotates "(Future: not yet implemented)". Enumeration uses a non-attach callback (FR-006) so no probe/claim step exists to skip. Not invented, not removed, no ALIGN task. Residual present-tense wording in User Story 1 Acceptance Scenario 4 / Edge Case remains tracked as align-tasks.md Task 4. |

## Verification

- Backups exist for both edited spec Markdown files under
  `.specify/sync/backups/specs/002-spdk-env-vfio-init/`.
- Workspace layout confirmed prior to backfill: `lib/spdk-sys` and
  `lib/component-framework` present; `components/spdk-sys` and
  `components/component-framework` absent; `components/spdk-env` present with
  `src/` and a `Cargo.toml` wiring both crates via `.workspace = true`.
- No `.rs` files modified; no cargo invoked.
