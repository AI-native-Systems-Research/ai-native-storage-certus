# Spec-Sync Proposals: example-helloworld-dylib

**Generated**: 2026-09-02T21:39:11Z
**Based on**: `.specify/sync/drift-report.{json,md}` (2026-09-02 re-analysis at HEAD 2fc1cd3c)

## Summary

| Direction | Count |
|-----------|-------|
| BACKFILL | 0 |
| ALIGN | 1 |
| HUMAN_DECISION | 0 |

Drift report: 7 requirements checked, 6 aligned, 1 drifted (FR-4, moderate), 0 not-implemented, 0 unspecced.

## Proposal 1 — FR-4 / Overview → **ALIGN** (approved)

- **Requirement**: FR-4 — "TypeId consistency is maintained by compiling both the dylib and host against the same `component-core`/`example-helloworld` source with the same `rustc` version (each side statically embeds its own copy; no shared `.so` linkage is involved)".
- **Direction**: ALIGN (no spec change; no `.rs` edit — task queued).
- **Rationale**: The spec is **correct** — FR-4 and the Overview were deliberately corrected in the 2026-07-22 backfill to describe compile-time `TypeId` identity via static `rlib` embedding, not shared `.so` linkage. The runtime behavior satisfies FR-4. The drift is the reverse direction (spec→code): the source module doc comment `src/lib.rs:4-7` still asserts the dylib and host "dynamically link the same `component-core` and `example-helloworld` shared libraries", contradicting the corrected spec. Correct spec + stale/inaccurate code doc ⇒ not spec-lag (not BACKFILL); it is a code-side documentation defect against an agreed-correct spec ⇒ **ALIGN**. Per workflow constraints the `.rs` source is not edited; the fix is queued as a task.
- **Action**: Confirm `align-tasks.md` **Task 2** (rewrite `src/lib.rs:4-7` to state static rlib embedding + same-`rustc` compile-time `TypeId` identity; drop the "dynamically link the same shared libraries" wording). No `spec.md` edit.

## Notes

- No BACKFILL edits ⇒ no `spec.md` files modified ⇒ no new spec backups required this run.
- The host-app doc `apps/dynamic-loading-example/src/main.rs:7-9` repeats the same stale linkage claim; noted for consistency in the align task but not modified (outside this component's scope).
