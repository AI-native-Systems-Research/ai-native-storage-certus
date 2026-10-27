# Drift Resolution Proposals — dispatcher

Generated: 2026-10-27T04:27:01Z · Branch: `spec-sync/coldload-v3-s20261025` (HEAD `3921112a`, base `a905f89b`) · Based on: `drift-report.json` of the same run.

All proposals are BACKFILL (code → spec). Code is authoritative: commit `4bb69289` (store-time round-robin drive placement, recorded in the entry's SSD location) was accepted and measured. Approval: the user's sync request (`--interactive` approval); all applied. Previous proposals (2026-10-21) are in `backups/20261027T042701Z/`.

| Id | Requirement | Before | After |
|---|---|---|---|
| B-14 | SC-007 | with key-based drive selection. | store-time drive selection recorded in each entry's SSD location (+ sync note) |
| B-15 | FR-025 | recover_extent(key, offset, size) | recover_extent(key, loc, size), loc = encode_loc(i, extent.offset) |
| B-16 | FR-004 | record the SSD offset | record the entry's SSD location; whole object on drive device_index % N; encode_loc |
| B-17 | FR-009 | (drive source unstated) | extent freed on the drive decoded from the location (select_drive); invalid drive frees nothing |
| B-18 | FR-028 | (failure list) | adds promote_and_serve's invalid-drive IoError; batch_lookup/promote_to_memory_tier reject before reserving |
| B-19 | FR-040 | (grouping unstated) | grouped by recorded drive; invalid drive skipped silently |
| B-20 | US-11 narrative | grouped by target drive | grouped by the drive recorded in their SSD location |
| B-21 | Key Entity Dispatch Map Entry | ssd_offset | ssd_offset holds (drive << 56) | extent_offset, carried opaquely |
| B-22 | Key Entity Background Writer | A dedicated thread | one writer per drive; job runs on writer device_index % N |
| B-23 | spec header | - | Last Synced 2026-10-27 line |
| B-24 | data-model.md device_index | which data device to target | + chosen round-robin by next_store_drive, recorded in the location |
| D-1 | info/FLOW.svg | drive_idx = key % num_drives | drive = loc >> 56 (round-robin at store) |

No ALIGN proposals. Open items carried over unchanged in `align-tasks.md`: A-1…A-5, A-01, A-02, A-04, A-05, A-06, A-08, A-10, N-01.
