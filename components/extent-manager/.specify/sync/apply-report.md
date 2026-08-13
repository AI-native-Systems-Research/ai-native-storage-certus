# Sync Apply Report — extent-manager

**Date**: 2026-08-07T16:02:56Z
Based on: proposals 2026-08-07T16:02:56Z (drift-report 2026-08-07T15:31:39Z)
Backups: `.specify/sync/backups/20260807T160256Z/` (spec.md)
Branch: `sync/spec-drift-sweep-20260807` (all changes; nothing committed to `unstable`)

## Changes Made

### Specs Updated (BACKFILL — applied directly)

| Requirement | Change | Detail |
|-------------|--------|--------|
| Header | Added | "Last Synced 2026-08-07" note summarizing this sweep. |
| FR-030 | Modified | Corrected inverted wording → "enabled = issue an explicit metadata flush after checkpoint/format writes". |
| FR-032 | Modified | `capacity_bytes()` now documented as **usable** data capacity (excludes reserved in-device metadata region). |
| FR-036 | Modified | Clarified `data_base_lba` is caller-consumed config; component performs no data-device I/O. |
| Additional Support Surface | Added | Documented 4 previously-unspecced helpers (checkpoint telemetry, WriteHandle accessors, extended mock helpers, `BuddyAllocator::mark_allocated`). |

### Code Drafted/Applied on Branch (ALIGN — see `align-tasks.md`)

| Requirement | Direction | Status | Files |
|-------------|-----------|--------|-------|
| FR-030 | ALIGN (HIGH) | **Drafted + verified** | `interfaces/src/iblock_device.rs` (FlushSync/FlushDone), `extent-manager/src/block_io.rs` (`flush()`), `block-device-spdk-nvme/src/actor.rs` (`do_sync_flush` + dispatch arm) |
| FR-016 | ALIGN (Low) | **Applied** | `interfaces/src/iextent_manager.rs:244`, `extent-manager/README.md:13` (→ 30 seconds) |

### Verification

- `cargo build -p interfaces` — clean.
- `cargo build` (default members) — clean.
- `cargo build -p block-device-spdk-nvme` — clean.
- `cargo build -p extent-manager --features volatile_write_cache` — clean
  (**was a compile error before this sweep**).
- `cargo test -p extent-manager --features volatile_write_cache` — 16 passed.

### Not Applied

| Proposal | Reason |
|----------|--------|
| (none) | All five proposals approved and applied. |

## Remaining Follow-ups (queued, not done here)

1. Add a CI job building `--features volatile_write_cache` (needs an SPDK-capable
   runner). See `align-tasks.md` Task 1.
2. Hardware review of `do_sync_flush` (real `spdk_nvme_ns_cmd_flush`).
3. Refresh stale `plan.md`/`tasks.md`/`README.md` planning docs (block_device
   receptacle, v2/ path, CERTUSV5/v5) — separate docs pass.

## Next Steps

1. Review the updated spec and the drafted code changes on the branch.
2. Commit on `sync/spec-drift-sweep-20260807` (do NOT commit to `unstable`).
