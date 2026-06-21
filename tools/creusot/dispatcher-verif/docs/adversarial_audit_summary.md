# Adversarial Audit Summary (Dispatcher)

This note captures key adversarial findings from the dispatcher spec-vs-implementation review and aligns them with verification planning artifacts.

## Concrete spec/implementation gap to track

Spec requirement (User Story 2, Acceptance Scenario 4):

- Given a cache entry exists but with size mismatch,
- When `lookup(key, ipc_handle)` is called,
- Then an `InvalidParameter` error is returned.

Implementation observations:

1. `components/dispatch-map/src/lib.rs:143`
   - `lookup()` does not compare requested size vs stored entry size.
   - `LookupResult::MismatchSize` is therefore not generated on this path.
2. `components/dispatcher/src/lib.rs:1271` and `components/dispatcher/src/lib.rs:1647`
   - Memory copy path can use truncated size (`min(actual, requested)` style behavior), which enables silent partial-copy semantics instead of explicit mismatch failure.

Risk:

- Silent truncation can hide contract violations and produce hard-to-debug data integrity issues.

Expected remediation:

1. Enforce size equality before copy in dispatcher lookup path.
2. Return `InvalidParameter` on mismatch (no copy side effects).
3. Align dispatch-map lookup API/behavior so mismatch is explicit and testable.
4. Keep this requirement mapped in:
   - `docs/first_properties.md` (lookup size-match contract),
   - `docs/verif_plan.md` (lookup ensures mismatch => `InvalidParameter`),
   - `docs/property_coverage.md` and `docs/assumption_ledger.md`.

## Status

- This gap remains a high-priority finding for adversarial validation.
- Property and plan entries were updated to make the mismatch rule explicit for future Creusot proof obligations and regression tests.
