# Drift Resolution Proposals — spdk-env (Spec-Sync Phase B)

Generated: 2026-08-20
Based on: `components/spdk-env/.specify/sync/drift-report.json` (specs analyzed: 2;
requirements checked: 26; aligned: 24; drifted: 1; not_implemented: 1; unspecced: 0)

Policy: `.specify/sync/PHASE_B_POLICY.md`. No special per-component note for
`spdk-env`; each item classified by reading its `location` code.

## Summary

| Resolution Type | Count |
|-----------------|-------|
| BACKFILL (spec → code) | 1 |
| ALIGN (task, no code change) | 0 |
| BACKFILL-UNSPECCED | 0 |
| RESOLVED (already fixed) | 0 |
| HUMAN_DECISION | 0 |
| not_implemented handled (LEAVE + NOTE) | 1 |

## Proposals

### Proposal 1 — 002-spdk-env-vfio-init / `stale-crate-paths`

**Direction**: BACKFILL (spec → matches code)

**Rationale**: The drift is pure spec-lag, not a behavioral bug. Verified the
workspace layout: `spdk-sys` now lives at `lib/spdk-sys/` and
`component-framework` at `lib/component-framework/` (the old `components/spdk-sys`
and `components/component-framework` paths do not exist; `spdk-env` itself
remains at `components/spdk-env/`, and its `Cargo.toml` wires both crates via
`.workspace = true`, which resolves correctly). The code/layout is the intended
reality; the supporting spec docs still cite the pre-move paths. → BACKFILL the
docs to match. No `.rs` change, no ALIGN task.

Scope of stale references (per drift `location`):
- `tasks.md:22,24,34,35,36` — literal filesystem paths `components/spdk-sys/...`
  (T001, T003, T004, T005, T006). Now `lib/spdk-sys/...`.
- `spec.md:6` — historical **Input** quote references `../component-framework`
  (a relative filesystem path). Now `../../lib/component-framework`.
- `spec.md:99` (FR-001) and `spec.md:188` (Assumptions) — reference the **crate
  name** "component-framework", which is **unchanged and still resolves** as a
  workspace dependency. **No edit needed** (not a stale path).

**Before**
- `tasks.md` T001/T003/T004/T005/T006: `components/spdk-sys/...`
- `spec.md:6`: "...use the framework provided in ../component-framework. The
  component interface, ISPDKEnv, ..."

**After**
- `tasks.md` T001/T003/T004/T005/T006: `lib/spdk-sys/...` (T002/T003
  `components/spdk-env` retained — that crate did not move); added a dated
  backfill note under the Organization line.
- `spec.md:6`: historical quote preserved verbatim with a bracketed editorial
  note: "...provided in ../component-framework [spec-sync backfill 2026-08-20:
  the component-framework crate has since moved to ../../lib/component-framework;
  the crate name is unchanged and still resolves as a workspace dependency]. The
  component interface, ISPDKEnv, ...". Lines 99/188 left unchanged (crate-name
  references remain valid).

---

### Proposal 2 — 002-spdk-env-vfio-init / `FR-015` (not_implemented)

**Direction**: LEAVE + NOTE (not_implemented; not obsolete, not a code bug)

**Rationale**: FR-015 ("skip devices in use by another process, log a warning per
skipped device, return only successfully probed devices") originates from a
recorded Clarifications decision (Session 2026-04-07) and is genuinely intended
future behavior, not never-built structure to be struck. The spec text already
self-flags it: "(Future: not yet implemented. Currently all matching devices are
claimed; user must ensure exclusive access via system configuration.)". The
current enumeration path in `src/env.rs` uses a non-attach callback (returns
non-zero, per FR-006) so devices are never claimed during enumeration; there is
no probe-and-skip step yet. Per policy, this is "genuinely needed but missing →
leave and note, don't invent" — so no spec rewrite, no invented implementation,
and no ALIGN task (the code does not violate an agreed requirement; the spec
itself defers it).

**Note for maintainers** (carried forward, not resolved here): User Story 1
Acceptance Scenario 4 and the matching Edge Case still describe the skip-and-warn
behavior in the present tense, which reads as inconsistent with FR-015's "Future:
not yet implemented" caveat. This was already tracked as an informational item in
a prior sync run (`align-tasks.md` Task 4). Left unedited pending a decision to
either implement the skip logic or soften the scenario wording; not invented here.

**Before / After**: none (no spec text changed).
