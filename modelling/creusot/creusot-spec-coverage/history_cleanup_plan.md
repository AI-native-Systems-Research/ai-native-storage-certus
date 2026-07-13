# History Cleanup Plan

Purpose:
- Identify which `history/` files are now redundant because their important content has been migrated to maintained docs.

Maintained docs that now carry core truth:
- `properties_to_prove.md`
- `assumptions_and_trusted.md`
- `coverage/coverage_report.md`
- `coverage/spec_drift_report.md`

## Safe to remove now (content superseded)

1. `history/assumption_ledger.md`
2. `history/trusted_ledger.md`
3. `history/property_coverage_matrix.md`
4. `history/coverage_gap_report.md`
5. `history/property_change_log.md`
6. `history/spec_inventory.md`
7. `history/Summary_of_md_files.md`
8. `history/terminology_spec_to_code.md`
9. `history/README.md` (only if `history/` folder is kept empty or removed)

Rationale:
- Key information from these files has been merged and clarified in maintained docs.
- Removing them does not remove current proof status or assumption/trust context.

## Keep short-term (1-2 review cycles), then remove

1. `history/property_coverage_dispatcher_july7.md`

Rationale:
- This is the most detailed per-proof chronology from Claude.
- We already migrated its essential facts to maintained docs, but keeping it briefly helps spot-check migration fidelity.

## Keep only if needed for archaeology

1. `history/first_properties.md`
2. `history/extracted_truths_from_spec.md`
3. `history/properties_based_on_truths.md`
4. `history/verif_plan.md`

Rationale:
- These are useful for historical narrative and methodology evolution, but no longer required for current operational workflow.

## Deletion order

1. Remove "Safe to remove now" group.
2. After one team review, remove `property_coverage_dispatcher_july7.md`.
3. Decide whether to retain archaeology set in a separate archive branch/folder.

## Document Evolution Summary

- Created after final migration pass to prevent accidental loss of important detail.
- Current recommendation is conservative: no immediate deletion of the Claude July deep log until one review cycle completes.
