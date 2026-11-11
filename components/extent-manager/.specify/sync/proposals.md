# Drift Resolution Proposals — extent-manager

Generated: 2026-11-11T21:14:07Z · Branch `copilot/fix-dispatcher-lba-issues` · HEAD `adacb8ce1` · Mode: interactive

| Resolution Type | Count |
|---|---|
| Backfill (Code → Spec) | 2 |
| Align (Spec → Code) | 0 |
| Human Decision | 0 |
| New Specs | 0 |

## P-EM1: FR-002, FR-032, FR-035, FR-036

**Direction**: BACKFILL · **Confidence**: HIGH

- Spec says: data_base_lba is caller-consumed only; metadata_region_size>0 always places data after the metadata area; capacity = data size minus metadata region
- Code does: nonzero data_base_lba selects a partitioned layout: metadata_region_size must be >0 (bounds the metadata partition), data_start_offset=0, capacity = full data_disk_size
- Evidence: `src/lib.rs:402-407`, `src/lib.rs:446-452`, `src/lib.rs:501-503`

**Action**: [x] Approve — Approved interactively by the user in /component-sync-specs --interactive, 2026-11-11.

## P-EM2: FR-004, Superblock entity, On-disk table, US3 AS5-AS6, plan.md

**Direction**: BACKFILL · **Confidence**: HIGH

- Spec says: magic CERTUSV4, version 6; initialize validates magic + CRC
- Code does: version 7; CERTUSP7 magic for partitioned formats; recovery rejects a layout/addressing-mode mismatch with a 'reformat' CorruptMetadata; unpartitioned CERTUSV4 of any version still readable
- Evidence: `src/superblock.rs:6-7`, `src/superblock.rs:117`, `src/recovery.rs:18-26`, `tests/checkpoint.rs:116`, `tests/checkpoint.rs:159`

**Action**: [x] Approve — Approved interactively by the user in /component-sync-specs --interactive, 2026-11-11.
