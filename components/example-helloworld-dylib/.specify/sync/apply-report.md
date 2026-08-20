# Spec-Sync Apply Report — example-helloworld-dylib

**Mode**: AUTO-BACKFILL
**Applied**: 2026-07-22
**Source**: `.specify/sync/drift-report.{json,md}`

## Summary

| Action | Count |
|---|---|
| Backfilled (spec corrected to match code) | 1 |
| New specs created | 0 |
| Superseded | 0 |
| Companion docs fixed | 1 (plan.md) + 1 (tasks.md) |
| Deferred to align-tasks.md | 1 |

## Backfilled

1. **FR-4 / Overview** (`specs/001-example-helloworld-dylib/spec.md`) — The spec claimed `TypeId` consistency across the dylib boundary is maintained "by dynamically linking shared dependencies" (`component-core`, `example-helloworld` as shared `.so` files). A clean rebuild + `ldd`/`nm` check showed neither crate declares `crate-type = ["dylib"]`; both the dylib and the host binary statically embed independent `rlib` copies. Corrected the Overview paragraph and FR-4 to state the actual mechanism: compile-time type identity from matching `rustc` version + same crate source (already partially captured by NFR-2). Backup of the pre-edit file saved to `.specify/sync/backups/spec.md.bak`.

## Companion Docs Fixed

- `specs/001-example-helloworld-dylib/plan.md` — Corrected the Technical Context "ABI strategy" bullet, the Architecture diagram's "shared deps" line, and Key Design Decision #1 to remove the false "shared dynamic linking" claim and describe the actual static-rlib-embedding behavior. Backup saved to `.specify/sync/backups/plan.md.bak`.
- `specs/001-example-helloworld-dylib/tasks.md` — Checked off the "Confirm `ComponentRef` return type matches the latest `component-core` API" item (verified aligned by drift analysis) and annotated the integration-test checklist item to point at the new align-tasks.md entry. Added a dated Spec-Sync Note summarizing the correction. Backup saved to `.specify/sync/backups/tasks.md.bak`.

## New Specs

None. No unspecced real production features were found (`drift-report.json`: `unspecced: []`).

## Superseded

None.

## Deferred to align-tasks.md

1. **Low | 001-example-helloworld-dylib (Task 1) | Missing automated integration test for the dylib-loading path** — `apps/dynamic-loading-example` is a manual demo binary with no `#[test]` coverage under `cargo test`; this is a code/test gap, not a spec text issue, so it was routed to `.specify/sync/align-tasks.md` rather than edited into the spec.

## Notes

- Status field on `spec.md` left as `Backfilled` (not flipped to `Reviewed`) since only one of the five human-review checklist items in `tasks.md` has been resolved by this pass; full human review is still pending.
- The `cdylib` variant decision (`tasks.md` checklist item 4) is an open design choice, not a drift finding — left untouched.
- Source code (`src/lib.rs`, `Cargo.toml`) was not modified; all changes were confined to `specs/**` and `.specify/sync/**` under this component.

---

# 2026-08-07 Sweep (branch `sync/spec-drift-sweep-20260807`)

Mode: sweep re-analysis. Pacing: auto-apply safe BACKFILL on-branch; queue
non-HIGH ALIGN items as align-tasks (do not draft). No forks arose.

Regenerated drift report (2026-08-07T15:29:55Z): 7 aligned, 1 drifted (Low,
doc-only), 0 not-implemented, 0 unspecced.

## No spec BACKFILL needed

The spec text is already correct — FR-4/Overview were backfilled to the
static-rlib-embedding mechanism in the 2026-07-22 run. The single remaining
drift is the reverse direction: the **source** module doc comment
(`src/lib.rs:4-7`) still carries the old "dynamically link the same shared
libraries" wording the spec has since corrected. That is an ALIGN (spec→code)
item, not a BACKFILL.

## Queued to align-tasks.md (non-HIGH ALIGN — queued, not drafted)

| Task | Severity | Summary |
|------|----------|---------|
| Task 2 — Align FR-4 stale module doc comment | Low (doc-only) | Rewrite `src/lib.rs:4-7` to state each side statically embeds its own rlib and that `TypeId` equality is a same-`rustc` compile-time property; drop the "dynamically link the same shared libraries" claim. No logic change. Full task + acceptance criteria appended to `align-tasks.md`. |

Task 1 (missing automated dylib-loading integration test, Low) from the
2026-07-22 run remains open.

## Verification
- No files edited under `specs/**` this sweep (spec already aligned). Only `.specify/sync/align-tasks.md` appended (Task 2). No `.rs` source modified.

---

# 2026-08-20 Phase B (per `.specify/sync/PHASE_B_POLICY.md`)

**Mode**: Phase B resolution (single component, 1 drift).
**Based on**: `.specify/sync/drift-report.{json,md}` (2026-08-20T09:24 regeneration) — 7 checked, 6 aligned, 1 drifted (FR-4, moderate), 0 not-implemented, 0 unspecced.

## Specs Updated

| Requirement | Change Type |
|-------------|-------------|
| (none) | — |

No `spec.md` was edited: FR-4 / Overview are already correct (static-rlib-embedding mechanism; no shared `.so`), so no BACKFILL was warranted.

## Align Tasks Generated

| Spec | Requirement | Task | Severity | Status |
|------|-------------|------|----------|--------|
| 001-example-helloworld-dylib | FR-4 / Overview | `align-tasks.md` Task 2 — correct stale module doc comment `src/lib.rs:4-7` | Moderate | Open (pre-existing; covers FR-4 exactly, re-confirmed this phase) |

FR-4 is spec→code drift: the spec is correct and behavior satisfies it, but the source doc comment still claims "dynamically link the same shared libraries". Classified ALIGN (not spec-lag). Per policy the `.rs` source is not edited; the doc fix is queued.

## Unspecced Backfilled

| Feature | Change |
|---------|--------|
| (none) | — |

## Resolved

| Item | Note |
|------|------|
| (none) | — |

## Backups

- **Spec `.md` files edited this phase: 0 ⇒ no new backups required.** Pre-existing backups from the 2026-07-22 backfill remain intact: `backups/spec.md.bak`, `backups/plan.md.bak`, `backups/tasks.md.bak`.

## Verification
- Edits confined to `.specify/sync/` (`proposals.md`, `proposals.json`, `apply-report.md`, `apply-report.json`, `align-tasks.md` note). No `specs/**` files changed (spec already correct). No `.rs` source modified; no cargo run.
