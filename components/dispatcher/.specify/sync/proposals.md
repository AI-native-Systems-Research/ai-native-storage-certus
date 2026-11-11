# Drift Resolution Proposals — dispatcher

Generated: 2026-11-11T21:14:07Z · Branch `copilot/fix-dispatcher-lba-issues` · HEAD `adacb8ce1` · Mode: interactive

| Resolution Type | Count |
|---|---|
| Backfill (Code → Spec) | 1 |
| Align (Spec → Code) | 0 |
| Human Decision | 0 |
| New Specs | 0 |

## B-53: US13 AS1, FR-062, FR-064 (new)

**Direction**: BACKFILL · **Confidence**: HIGH

- Spec says: read_to_bounce reads at LBA raw_offset / block_size; partition-relative data I/O unspecified
- Code does: every data I/O path addresses data_base_lba + extent_offset / block_size; recorded locations stay relative; format bounds metadata_region_size to the metadata partition; pre-change drives must be reformatted
- Evidence: `src/lib.rs:935`, `src/lib.rs:1100`, `src/lib.rs:1686`, `src/lib.rs:2088`, `src/lib.rs:2922`, `src/lib.rs:4354`, `src/lib.rs:4706`, `src/lib.rs:8536`, `src/repair_test_flush_failed_write.rs:622`

**Action**: [x] Approve — Approved interactively by the user in /component-sync-specs --interactive, 2026-11-11.
