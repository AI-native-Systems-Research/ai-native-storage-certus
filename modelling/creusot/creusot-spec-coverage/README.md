# Creusot Spec Coverage (Simplified)

This folder tracks how we turn Certus specs into concrete Creusot proof targets, with enough detail to stand on its own (even if `history/` is later trimmed).

## Folder layout

Maintained source docs:
- `properties_to_prove.md`:
  Property baseline with status, owner, abstraction level, and artifact evidence pointers.
- `assumptions_and_trusted.md`:
  Assumption/trust ledger linked to affected property IDs.

Coverage reports:
- `coverage/coverage_report.md`:
  One-screen dashboard — counts by status / abstraction level / component, plus the ownership-API table.
- `coverage/proof_locator.md`:
  Per-property pointer: where each Px proof lives (crate, function, `.coma`) and one line on what it proves. Answers "where is the proof for Px?" and "what does function `<odd-name>` do?".
- `coverage/spec_drift_report.md`:
  Spec drift, ownership mapping, and status-change rationale.

Archive:
- `history/`:
  Previous snapshots and intermediate files (kept for audit trail).

## Core terms (plain English)

- **Property**: a precise behavior claim we want to prove.
- **Owner**: interface/component that should provide the proof evidence.
- **Verified**: proof exists and still mirrors active code/spec.
- **Partial**: some supporting proofs exist but full scope not discharged.
- **Unchecked**: no sufficient proof yet.
- **Stale**: artifact exists, but path changed/removed so it no longer proves current runtime behavior.
- **Retired**: property removed from active scope due to spec/runtime changes.

## Workflow

1. Update `properties_to_prove.md` from current specs.
2. Update `assumptions_and_trusted.md` for modeling/trust changes.
3. Refresh coverage reports under `coverage/`.
4. Keep older snapshots under `history/` (do not rely on history for critical active status).

## Document Evolution Summary

- Simplified to 2 maintained source docs + 3 coverage reports (dashboard, proof locator, spec drift).
- Active docs now include concrete proof artifact references and Claude-July stale/live transitions.
- Goal: readers can answer “what is proved, where, and under what assumptions” without opening archived files.
