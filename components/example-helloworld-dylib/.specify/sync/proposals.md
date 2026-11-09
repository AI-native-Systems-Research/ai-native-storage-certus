# Spec-Sync Phase B — Proposals: example-helloworld-dylib

**Generated**: 2026-08-20
**Based on**: `.specify/sync/drift-report.{json,md}` (2026-08-20T09:24 regeneration)
**Policy**: `.specify/sync/PHASE_B_POLICY.md`

## Summary

| Direction | Count |
|-----------|-------|
| BACKFILL | 0 |
| ALIGN | 1 |
| BACKFILL-UNSPECCED | 0 |
| RESOLVED | 0 |
| HUMAN_DECISION | 0 |

Drift report: 7 requirements checked, 6 aligned, 1 drifted (FR-4, moderate), 0 not-implemented, 0 unspecced.

## Proposal 1 — FR-4 / Overview → **ALIGN**

- **Requirement**: FR-4 — "TypeId consistency is maintained by compiling both the dylib and host against the same `component-core`/`example-helloworld` source with the same `rustc` version (each side statically embeds its own copy; no shared `.so` linkage is involved)".
- **Direction**: ALIGN (no spec change; no `.rs` edit — task queued).
- **Rationale**: The spec is **correct** — FR-4 and the Overview were deliberately corrected in the 2026-07-22 backfill to describe compile-time `TypeId` identity via static `rlib` embedding, not shared `.so` linkage. The runtime behavior satisfies FR-4 (the drift report marks the requirement itself as satisfied). The drift is the reverse direction (spec→code): the source module doc comment `src/lib.rs:4-7` still asserts the dylib and host "dynamically link the same `component-core` and `example-helloworld` shared libraries", contradicting the corrected spec. Correct spec + stale/inaccurate code doc ⇒ this is **not** spec-lag (so not BACKFILL); it is a real code-side documentation defect against an agreed-correct spec ⇒ **ALIGN**. Per policy the `.rs` source is not edited; the fix is queued as a task.
- **Before (spec.md)**: unchanged — FR-4 already reads correctly (see requirement text above).
- **After (spec.md)**: unchanged — no BACKFILL applied. Code-side fix queued as `align-tasks.md` **Task 2** (rewrite `src/lib.rs:4-7` to state static rlib embedding + same-`rustc` compile-time `TypeId` identity; drop the "dynamically link the same shared libraries" wording).

## Notes

- No BACKFILL edits ⇒ no `spec.md` files were modified ⇒ **zero new spec backups required** this phase. (Prior backups from the 2026-07-22 backfill remain under `.specify/sync/backups/`.)
- The host-app doc `apps/dynamic-loading-example/src/main.rs:6-9` repeats the same stale linkage claim; noted for consistency in the align task but not modified (outside this component's scope).
