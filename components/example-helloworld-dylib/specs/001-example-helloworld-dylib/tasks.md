# Tasks

## Review Backfilled Spec

- [ ] Verify that the spec accurately reflects the current implementation intent
- [x] Confirm `ComponentRef` return type matches the latest `component-core` API — confirmed via spec-sync analysis (2026-07-22): `ComponentRef::from(Arc<_>)` matches current `component-core` usage.
- [ ] Validate that the dylib is exercised by at least one integration test in the workspace — tracked as a code gap in `.specify/sync/align-tasks.md` (Task 1), not a spec issue.
- [ ] Decide whether to add a `cdylib` variant for cross-compiler-version compatibility
- [ ] Update spec status from "Backfilled" to "Reviewed" once validated

## Spec-Sync Note (2026-07-22)

`spec.md` (Overview, FR-4) and `plan.md` (Architecture diagram, Key Design Decision #1) were corrected during spec-sync AUTO-BACKFILL: the original text claimed `component-core`/`example-helloworld` are dynamically linked as shared `.so` dependencies between the dylib and host. A clean-rebuild `ldd`/`nm` check showed both sides statically embed independent `rlib` copies instead; `TypeId` consistency actually comes from compiling against the same source with the same `rustc` version (already captured by NFR-2). See `.specify/sync/drift-report.md` for full evidence.
