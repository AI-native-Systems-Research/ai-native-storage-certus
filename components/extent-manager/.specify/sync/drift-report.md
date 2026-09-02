---
spec_sync_component: extent-manager
spec_sync_drift_status: clean
spec_sync_synced_at: 2026-09-02T20:45:00Z
spec_sync_git_commit: 787b8263
spec_sync_inputs_sha256: 5e8ff0212e33c827010f5b927a6f85f53cc28fbbfb74c4878170acf5294622da
spec_sync_hash_tool: scripts/spec-sync-hash.sh
---
# Spec ↔ Implementation Drift Report — extent-manager

**Generated**: 2026-09-01T22:59:04Z (re-stamped 2026-09-02 for an interfaces-only hash change)
**Spec**: `specs/001-extent-manager-v2/spec.md` (Updated 2026-08-20)
**Commit**: 787b8263

> **2026-09-02 re-stamp (no content change).** This component's own `src/` and
> `specs/` are byte-for-byte unchanged since its `fbc2fc24` sync, which stamped
> the correct hash for that tree. The freshness hash folds in all of
> `components/interfaces/src/**`, and branch `evolve-dispatcher-dw` edited
> `idispatcher.rs`, `igpu_services.rs`, and the `lib.rs` re-exports
> (`GpuMemcpyBatchOp`/`GpuMemcpySrcAccessOrder`). extent-manager references none
> of those (it implements `IExtentManager` and consumes `IBlockDevice`), so there
> is no new drift — only the folded-in hash moved
> (`710da42e…` → `5e8ff021…`). Re-stamped so the CI Spec-Sync Gate sees a fresh
> report; the drift analysis below still holds verbatim.

## Summary

| Metric | Count |
|--------|-------|
| Specs Analyzed | 1 |
| Requirements Checked | 37 FR + 6 SC |
| Aligned | 42 |
| Drifted | 1 (spec) + 1 (plan doc) |
| Not Implemented | 0 |
| Unspecced | 0 |

The spec was generated from implementation and last synced 2026-08-20 (Phase B).
That sync already resolved the previously-reported principal drift — the stale
FR-030 "does not compile" note — so relative to the **current** spec the code is
almost entirely in sync. This fresh pass found the flush surface fully landed and
one residual over-claim in FR-030, plus stale historical/planning text.

## Spec: 001-extent-manager-v2 — Extent Manager V2

### Aligned ✓

| Req | Evidence |
|-----|----------|
| FR-001 define_component ExtentManager, metadata_device + logger receptacles | `src/lib.rs:81-103` |
| FR-002 format validates params (sector>0, slab%sector, max<=slab, region pow2, md size) | `src/lib.rs:384-401,434-438` |
| FR-003 superblock at LBA 0 w/ CRC32, magic CERTUSV4, version 6 | `src/superblock.rs:4-6,58-97`; written `src/lib.rs:496-498` |
| FR-004 initialize reads sb + recovers active checkpoint + rebuilds state | `src/lib.rs:514-582` |
| FR-005 reserve_extent → sector-aligned WriteHandle, not visible until publish | `src/lib.rs:584-630` |
| FR-006 publish writes key; FREE_KEY → immediate free + Ok, no entry | `src/lib.rs:597-616` |
| FR-007 abort/drop releases slot | `src/lib.rs:618-621`; `iextent_manager.rs:141-155` RAII |
| FR-008 remove_extent → FREE_KEY, deferred free, OffsetNotFound | `src/lib.rs:680-684`; `region.rs:121-160` |
| FR-009 get_extents / for_each_extent skip FREE_KEY | `src/lib.rs:632-678` |
| FR-010/011/012 dense key `Vec<u64>`, FREE_KEY=u64::MAX sentinel, BTreeMap by start_offset | `src/slab.rs:5,12,24,46-47`; `region.rs:16` (`slabs`), lookup `region.rs:121-147` |
| FR-013 checkpoint serializes slabs+key vectors, CRC, switch active | `src/lib.rs:299-307`; `checkpoint::write_checkpoint` |
| FR-014 skip I/O when clean | `src/lib.rs:276-286` (`any_dirty`) |
| FR-015 coalesced concurrent checkpoints (single writer) | `src/lib.rs:729-761` (seq + in_progress + condvar) |
| FR-016 background thread @ 30s default | `src/lib.rs:109-112,120-164`; docs corrected: `iextent_manager.rs:205`, `README.md:13` |
| FR-017 dual-copy fallback (active then inactive) | `src/recovery.rs` (`recover`) |
| FR-018 recover from key vectors, mark allocated | `src/lib.rs:552-565` (`buddy.mark_allocated`) |
| FR-019 buddy allocator per region | `src/lib.rs:465`; `src/buddy.rs` |
| FR-020 bitmap slab allocator w/ rover | `src/slab.rs`, `src/bitmap.rs` |
| FR-021 size-class manager (HashMap by element_size) | `src/lib.rs:563`; `slab.rs:92` (`SizeClassManager.map`) |
| FR-022 key sharded by `key & (region_count-1)` | `src/lib.rs:219` |
| FR-023 per-region parking_lot::RwLock | `src/lib.rs:90`; `Arc<RwLock<RegionState>>` |
| FR-024 Send+Sync (composed, not compile-enforced, as specced) | types compose |
| FR-025 deferred free until checkpoint | `src/region.rs:147,152-160`; `src/lib.rs:318` (`flush_pending_frees`) |
| FR-026 get_instance_id | `src/lib.rs:686-692` |
| FR-027 set_checkpoint_interval(None disables) | `src/lib.rs:694-696,132-141` |
| FR-028 set_metadata_ns_id (concrete only, not in trait) | `src/lib.rs:181-183`; absent from `iextent_manager.rs:170-223` trait |
| FR-029 set_dma_alloc | `src/lib.rs:169-171` |
| FR-031 used_bytes (buddy granularity) | `src/lib.rs:698-708`; trait `iextent_manager.rs:209` |
| FR-032 capacity_bytes = usable data | `src/lib.rs:710-715`; trait `iextent_manager.rs:212` |
| FR-033 WriteHandle in interfaces crate, RAII two-phase | `interfaces/src/iextent_manager.rs:95-155` |
| FR-034 fault injection behind testing feature | `src/test_support.rs:16-72` (`FaultConfig`, `fail_after_n_writes`, `fail_all_writes`) |
| FR-035 metadata_region_size shared-device mode, default 128 MiB | `src/lib.rs:425-446`; `iextent_manager.rs:66,90` |
| FR-036 metadata/data base_lba methods on trait | `src/lib.rs:717-727`; trait `iextent_manager.rs:215-221` |
| FR-037 set_post_checkpoint_hook fires once, synchronously, after checkpoint | `src/lib.rs:173-175,365-367` |
| SC-001..006 | unit/integration tests `tests/{lifecycle,checkpoint,concurrent,edge_cases}.rs` |

Additional Support Surface (documented 2026-08-07) still matches code: checkpoint
telemetry `checkpoint_complete` (`lib.rs:360-363`), WriteHandle read accessors
`key()/extent_offset()/extent_size()` (`iextent_manager.rs:120-130`), extended mock
helpers (`test_support.rs:44-72`), `BuddyAllocator::mark_allocated` (`buddy.rs`).

### Drifted ⚠️

- **FR-030 (`volatile_write_cache`) — "(and in the format path)" over-claims —
  moderate (spec-side).** FR-030's prose states the component issues a flush
  "to the metadata device after checkpoint writes **(and in the format path)**".
  The flush surface has fully landed and is real: `BlockDeviceClient::flush()`
  (`src/block_io.rs:167-180`) sends `Command::FlushSync` / awaits
  `Completion::FlushDone` (interfaces `iblock_device.rs:411,501`). But the only
  call site is the **checkpoint** path (`src/lib.rs:308-310`, under
  `#[cfg(feature = "volatile_write_cache")]`). `format()` writes the superblock
  at `src/lib.rs:496-498` with **no** flush. The code's own supporting artifacts
  agree it is checkpoint-only: `README.md:66` ("flush after checkpoint writes")
  and `Cargo.toml:24-27`. Recommend correcting the FR-030 prose to drop the
  "(and in the format path)" claim so the spec matches the shipped behavior. The
  FR-030 implementation-status note already scopes the call site correctly to the
  checkpoint path — only the leading prose is inconsistent. (Alternative, if
  format-time durability is actually desired: add a `flush()` after
  `lib.rs:498` — but that is a behavior change, not a doc sync; flagged, not
  assumed.)

- **FR-016 stale remediation note — minor (spec-side, doc).** FR-016 still carries
  present-tense remediation language: the interface doc comment and README
  "incorrectly state 'five minutes' … stale doc strings that should be corrected".
  That correction has already landed — `iextent_manager.rs:205` and `README.md:13`
  both now say 30 seconds. The note describes a defect that no longer exists and
  should be rewritten to simply state the 30-second default (dropping the
  "incorrectly state / should be corrected" wording).

### Not Implemented ✗

None.

## Unspecced Features

None. The four previously-unspecced helpers were folded into the spec's
"Additional Support Surface" section on 2026-08-07 and still match code.

## Planning-Doc Drift (informational)

`plan.md` was refreshed 2026-08-20 and now correctly shows only
`metadata_device` + `logger` receptacles and a flat `src/`. Two stale factual
details remain in the diagrams:

- `plan.md:59` — `checkpoint_interval_ms: AtomicU64 (default 5000)`. The shipped
  mechanism is `checkpoint_timer_state: Arc<CheckpointTimerState>` (a
  `Mutex<Option<Duration>>` + `Condvar` + shutdown flag, `lib.rs:62-79,95`) with a
  **30 s** default (`lib.rs:112`), not a 5 s `AtomicU64`.
- `plan.md:229` — describes `superblock.rs` as "(v5)"; actual `FORMAT_VERSION = 6`
  (`superblock.rs:6`), matching spec.

These are planning-doc cosmetics; they do not affect the spec↔code contract.

## Recommendations

1. FR-030: remove the "(and in the format path)" parenthetical from the prose so
   it matches the checkpoint-only flush that is actually implemented (corroborated
   by README + Cargo.toml). Do **not** silently add a format-path flush — surface
   that separately if format-time durability is wanted.
2. FR-016: replace the stale "incorrectly state 'five minutes' / should be
   corrected" remediation note with a plain statement of the 30 s default.
3. `plan.md`: fix `AtomicU64 (default 5000)` → `CheckpointTimerState` @ 30 s, and
   `(v5)` → `(v6)`.
