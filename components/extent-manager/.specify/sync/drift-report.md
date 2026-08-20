# Spec ↔ Implementation Drift Report — extent-manager

**Generated**: pending

## Summary

| Metric | Count |
|--------|-------|
| Specs Analyzed | 1 |
| Requirements Checked | 37 FR + 6 SC |
| Aligned | 41 |
| Drifted | 2 |
| Not Implemented | 0 |
| Unspecced | 0 |

Spec is largely in sync (it was generated from implementation and last synced
2026-08-07). The principal drift is a **stale implementation-status note for
FR-030** (`volatile_write_cache`), which the spec still says "does not compile"
even though the required flush surface now exists in code and interfaces.

## Spec: 001-extent-manager-v2 — Extent Manager V2

### Aligned ✓

| Req | Evidence |
|-----|----------|
| FR-001 define_component ExtentManager, metadata_device + logger receptacles | `src/lib.rs:81-103` |
| FR-002 format validates params | `src/lib.rs:383-401` |
| FR-003 superblock at LBA 0 w/ CRC | `src/lib.rs:483-498`; `Superblock::new/serialize` |
| FR-004 initialize reads sb + recovers | `src/lib.rs:514-582` |
| FR-005 reserve_extent → sector-aligned WriteHandle, not visible until publish | `src/lib.rs:584-630` |
| FR-006 publish writes key; FREE_KEY → free + Ok | `src/lib.rs:597-616` |
| FR-007 abort/drop releases slot | `src/lib.rs:618-621`; `WriteHandle` RAII |
| FR-008 remove_extent → FREE_KEY, deferred free, OffsetNotFound | `src/lib.rs:680-684`; `region.rs::remove_extent_by_offset` |
| FR-009 get_extents / for_each_extent skip FREE_KEY | `src/lib.rs:632-678` |
| FR-010/011/012 slab key vectors, FREE_KEY sentinel, BTreeMap by start_offset | `src/slab.rs`, `src/region.rs` (get_key/slot iteration `src/lib.rs:639-648`) |
| FR-013 checkpoint serializes slabs+key vectors, CRC, switch active | `src/lib.rs:299-307`; `checkpoint::write_checkpoint` |
| FR-014 skip I/O when clean | `src/lib.rs:276-286` (`any_dirty`) |
| FR-015 coalesced concurrent checkpoints | `src/lib.rs:729-761` (seq + in_progress + condvar) |
| FR-016 background thread @ 30s default | `src/lib.rs:109-112,120-164` |
| FR-017 dual-copy fallback | `recovery::recover` (active then inactive) |
| FR-018 recover from key vectors, mark allocated | `src/lib.rs:552-565` (`mark_allocated`) |
| FR-019 buddy allocator per region | `src/lib.rs:465`; `src/buddy.rs` |
| FR-020 bitmap slab allocator w/ rover | `src/slab.rs`, `src/bitmap.rs` |
| FR-021 size-class manager | `src/lib.rs:563`; `region.size_classes` |
| FR-022 key sharded by key & (region_count-1) | `src/lib.rs:219` |
| FR-023 per-region parking_lot::RwLock | `src/lib.rs:90`; `Arc<RwLock<RegionState>>` |
| FR-024 Send+Sync | composes; no compile enforcement (as specced) |
| FR-025 deferred free until checkpoint | `flush_pending_frees` `src/lib.rs:318` |
| FR-026 get_instance_id | `src/lib.rs:686-692` |
| FR-027 set_checkpoint_interval(None disables) | `src/lib.rs:694-696,132-141` |
| FR-028 set_metadata_ns_id (concrete only) | `src/lib.rs:181-183` |
| FR-029 set_dma_alloc | `src/lib.rs:169-171` |
| FR-031 used_bytes (buddy granularity) | `src/lib.rs:698-708` |
| FR-032 capacity_bytes = usable data | `src/lib.rs:710-715` |
| FR-033 WriteHandle in interfaces crate | `interfaces` import `src/lib.rs:25-28` |
| FR-034 fault injection behind testing feature | `src/test_support.rs` |
| FR-035 metadata_region_size shared-device mode | `src/lib.rs:425-446` |
| FR-036 metadata/data base_lba methods | `src/lib.rs:717-727` |
| FR-037 set_post_checkpoint_hook fires once after checkpoint | `src/lib.rs:173-175,365-367` |
| FR-016 "five minutes" remediation | interfaces doc + README now say 30s (`iextent_manager.rs:244`, `README.md:13`) |
| SC-001..006 | unit/integration tests `tests/{lifecycle,checkpoint,concurrent,edge_cases}.rs` |

### Drifted ⚠️

- **FR-030 (`volatile_write_cache`) — implementation-status note is stale** —
  **moderate**. Spec (top Sync note + FR-030, dated 2026-08-07) states the
  feature "does not yet compile — `BlockDeviceClient::flush()` missing;
  `Command::FlushSync`/`Completion::FlushDone` absent from the interfaces crate."
  Those symbols now exist: `flush()` at `src/block_io.rs:168` uses
  `Command::FlushSync` / `Completion::FlushDone`; the interface defines
  `FlushSync` (`components/interfaces/src/iblock_device.rs:411`) and `FlushDone`
  (`:501`); `run_checkpoint` calls `metadata_client.flush()` under the feature
  (`src/lib.rs:308-310`). The spec's "does not compile" language and the queued
  align-task should be updated to reflect that the flush surface has landed.
  (Full `--features volatile_write_cache` build not run here per read-only scope.)

- **Planning docs reference stale layout** — **minor**. `plan.md` still cites a
  `block_device` receptacle (`plan.md:25,39`) and a `components/extent-manager/v2/`
  source path (`plan.md:213`) that do not exist — the shipped component has only
  `metadata_device` + `logger` and a flat `src/`. Spec.md's top note already flags
  this as informational; refresh `plan.md`/`tasks.md` to match.

### Not Implemented ✗

None.

## Unspecced Features

None outstanding. The four previously-unspecced helpers (checkpoint telemetry
log, WriteHandle read accessors, extended mock helpers,
`BuddyAllocator::mark_allocated`) were documented in the spec's "Additional
Support Surface" section on 2026-08-07 and match code
(`src/lib.rs:360-363`, `src/buddy.rs:117-157`, `src/test_support.rs`).

## Recommendations

1. Update FR-030 and the top Sync note: the `volatile_write_cache` flush surface
   now exists in code and interfaces; remove "does not compile" wording (verify
   with a `cargo build -p extent-manager --features volatile_write_cache` CI job).
2. Refresh `plan.md`/`tasks.md` to drop the obsolete `block_device` receptacle
   and `v2/` path references so planning docs agree with spec.md + code.
