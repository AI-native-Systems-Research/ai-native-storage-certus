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
