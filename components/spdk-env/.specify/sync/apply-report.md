# Spec-Sync Apply Report — spdk-env

**Applied**: 2026-09-02T21:46:08Z
**Source**: `components/spdk-env/.specify/sync/drift-report.{json,md}`,
`proposals.{json,md}`
**Git commit**: 2fc1cd3c
**Inputs sha256**: 8d1b1befdd776e12281e5c5d75a495e4b57500b039cda72761d72a2d82155584

## Scope

Read-only re-verification of the whole component against both specs
(`001-spdk-vfio-env` superseded scaffold; `002-spdk-env-vfio-init` live). Source
(`src/*.rs`) is unchanged since 2026-06-11, so every FR-001..FR-021 and
SC-001..SC-005 was independently re-checked against the code with file:line
evidence. Only files under `components/spdk-env/.specify/sync/**` were written
this run. **No spec `.md` was edited; no `.rs` source was touched; no cargo run.**

## Drift-report items and their resolution

| Item | Type in report | Direction | Outcome |
|------|----------------|-----------|---------|
| SC-001 / US1 / Clarifications device-type scope | drifted (medium) | HUMAN_DECISION | Left in report; carried as align Task 1 (OPEN) |
| FR-015 | not_implemented | LEAVE + NOTE | No edit; self-flagged "Future"; align Task 4 (OPEN) |
| stale-crate-paths | resolved | RESOLVED | Verified 2026-08-20 backfill still present; no action |

## Specs Updated (BACKFILL)

None this run. No confident spec→code backfill was available:
- The only live drift (SC-001 device-type scope) is a HUMAN_DECISION — narrowing
  a recorded clarification/product decision to match a partial implementation is
  out of scope for an auto-backfill.
- The stale-crate-paths backfill was already applied on 2026-08-20 and verified
  present (`tasks.md:10` + `lib/spdk-sys/` paths; `spec.md:6` bracketed note).

## Backups

None created — no spec `.md` files were edited this run. (Prior-run backups are
retained under `.specify/sync/backups/`.)

## Align Tasks Generated

No new code ALIGN tasks. A dated **2026-09-02** section was appended to
`align-tasks.md` re-confirming Task 1 (SC-001 device-type scope, OPEN) as the
live drift and Task 4 (FR-015 wording, OPEN) as the not-implemented note.

## Unspecced Backfilled

None (drift report lists 0 unspecced features).

## not_implemented Handled

| Requirement | Handling | Notes |
|-------------|----------|-------|
| FR-015 (skip in-use devices, warn per device, return only probed devices) | LEAVE + NOTE | Intended future behavior from Clarifications; spec self-annotates "(Future: not yet implemented)". Enumeration uses a non-attach callback (FR-006), so no probe/claim step exists to skip. Not invented, not removed, no ALIGN code task. Residual present-tense wording in US1 Acceptance Scenario 4 / Edge Case tracked as align Task 4. |

## Verification highlights (this run)

- FR-018: local `ISPDKEnv` (`src/lib.rs:34-56`) and mirror
  (`components/interfaces/src/ispdk_env.rs:5-27`) confirmed **identical** (same 5
  methods/signatures/docs).
- FR-020: `DmaBuffer` re-exported at `src/dma.rs:6`; backing impl in
  `interfaces/src/spdk_types.rs` (`new` :238, `from_raw` :293, `Deref`/`DerefMut`
  :392/:401, `drop` gated by `is_spdk_env_active()` :376-381). Active flag toggled
  by `env.rs:102` (true) / `env.rs:191` (false).
- FR-021: all 5 scripts present under `components/spdk-env/scripts/`.
- FR-015 still unimplemented (`env.rs:115-185` has no skip/warn path).
- SC-001 scope gap confirmed against `env.rs:164-181` (NVMe driver only).
