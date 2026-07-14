# Summary of MD Files in `creusot-spec-coverage`

This file explains why each markdown file exists, how it is used in the Creusot workflow, and where it should live under a 3-folder layout:
- `canonical/` = spec-to-proof source docs we rely on when writing/updating verification functions.
- `generated/` = machine-generated or refreshable reports.
- `explanatory/` = human guidance/status/context docs.

## Proposed classification

| File | Purpose | Why keep it | Classification |
|---|---|---|---|
| `first_properties.md` | Canonical property set (P1..Pn) extracted from spec. | Main target list for proofs and coverage checks. | `canonical/` |
| `extracted_truths_from_spec.md` | Requirement “truths” pulled from spec text. | Traceability layer from natural-language spec to formal properties. | `canonical/` |
| `properties_based_on_truths.md` | Mapping from properties to underlying truths. | Justifies each property and avoids arbitrary proof scope. | `canonical/` |
| `verif_plan.md` | Planned proof strategy and staging order. | Guides which Creusot functions/contracts are implemented next. | `canonical/` |
| `assumption_ledger.md` | Assumptions used by current proofs/models. | Makes proof limits explicit and reviewable. | `canonical/` |
| `trusted_ledger.md` | Trusted lemmas/boundaries and justification. | Tracks non-proved trust points for audit rigor. | `canonical/` |
| `terminology_spec_to_code.md` | Shared definitions (coverage, stale, mapping terms). | Keeps team language consistent across tools/people. | `explanatory/` |
| `property_coverage_matrix.md` | Property-by-property status baseline (Covered/Partial/Not covered). | Core coverage dashboard used by reviews and planning. | `generated/` (or `canonical/` if edited manually as source) |
| `coverage_gap_report.md` | Prioritized gap report derived from matrix/status docs. | Fast action list for next proof work. | `generated/` |
| `property_change_log.md` | Drift report vs baseline spec (added/removed/changed impacts). | Prevents stale proofs after spec evolution. | `generated/` |
| `spec_inventory.md` | Discovered component spec paths. | Ensures extraction runs over intended specs and catches new specs. | `generated/` |
| `property_coverage_dispatcher_july7.md` | Detailed dispatcher proof status narrative (`Verified/Unchecked/Stale/Retired`). | Rich context for reviewer decisions beyond matrix shorthand. | `explanatory/` |
| `README.md` | Folder contract and update process. | Onboarding and maintenance rules for contributors. | `explanatory/` |

## Notes

1. If we want strict reproducibility, treat `property_coverage_matrix.md` as generated from a canonical source (or clearly mark manual edits).
2. If we want strict human control, move `property_coverage_matrix.md` to `canonical/` and keep generated reports derived from it.
3. `property_coverage_dispatcher_july7.md` is date-stamped context; keep it in `explanatory/` or move to `explanatory/history/` as newer snapshots are created.
