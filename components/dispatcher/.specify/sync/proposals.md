# Drift Resolution Proposals — dispatcher

Generated: 2026-11-17T22:05:43Z · Branch `opt/evolve-connector-store-sustained-v5/s20261112-203027` · HEAD `b5cf747d1` · Mode: interactive

| Resolution Type | Count |
|---|---|
| Backfill (Code → Spec) | 2 |
| Align (Spec → Code) | 0 |
| Human Decision | 0 |
| New Specs | 0 |

## B-54: FR-039 step (4), SC-007, FR-004

**Direction**: BACKFILL · **Confidence**: HIGH

- Spec says: a store's drive is chosen round-robin per committing thread at `copy_gpu_to_memory_completed`/`schedule_write_through`
- Code does: an early write-through (FR-004 amendment) takes the drive from the same per-thread cursor at the block's copy sync, on the copying thread; one request's blocks still get consecutive drives
- Evidence: `src/lib.rs:1630-1641`, `src/lib.rs:666`, `src/lib.rs:4119`

**Action**: [x] Approve — Approved interactively by the user in /component-sync-specs --interactive, 2026-11-17.

## B-55: FR-018

**Direction**: BACKFILL · **Confidence**: HIGH

- Spec says: the write-through pin is recorded before the job is enqueued, so the job cannot start first
- Code does: for an early write-through the job is queued before the commit; the commit records the pin under the same `WriteThroughPins` lock, and the job settles it, or the commit settles a write that already landed
- Evidence: `src/background.rs:330-357`, `src/background.rs:421-445`

**Action**: [ ] Not approved in the interactive review, 2026-11-17: FR-018 left unchanged; drift remains open.

---

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
