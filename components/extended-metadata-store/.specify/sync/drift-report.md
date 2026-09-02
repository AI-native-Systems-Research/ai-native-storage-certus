---
spec_sync_component: extended-metadata-store
spec_sync_drift_status: drift
spec_sync_synced_at: 2026-09-02T21:41:34Z
spec_sync_git_commit: 2fc1cd3c
spec_sync_inputs_sha256: 97cefb4fa4fdbaf6628abf8f951c22ddfcbcf705d8dfb06ac8c25e269892373a
spec_sync_hash_tool: scripts/spec-sync-hash.sh
---

# Drift Report: extended-metadata-store

**Generated**: 2026-09-02 (sync sweep)
**Project**: extended-metadata-store
**Git commit**: 2fc1cd3c

## Summary

| Metric | Count |
|--------|-------|
| Specs Analyzed | 1 (`001-extended-metadata-store`) |
| Requirements Checked | 30 (18 FR + 11 NFR + implied SC) |
| Aligned | 27 |
| Drifted | 3 |
| Not Implemented | 0 |
| Unspecced Features | 0 new (3 previously backfilled — FR-18, NFR-11, CapacityExhausted note) |

Only `001-extended-metadata-store` exists in `specs/` this sweep. The prior report
analyzed a second spec, `002-ssd-integration-test`, which is **no longer present in
the repository** (absent from `specs/` and from git HEAD; only backups survive under
`.specify/sync/backups/`). Its SSD integration tests (`tests/integration_ssd.rs`)
still exist and are referenced from 001's User Stories. See the HUMAN_DECISION item
below.

The 2026-08-20 sweep's backfills into `spec.md` (FR-05 Implemented, FR-18, NFR-11,
RESOLVED notes for NFR-07 / workspace membership) were **verified against the current
code and are correct**. This sweep found additional drift the prior sweeps missed
(FR-11 dirty-threshold trigger) and stale sibling artifacts (`plan.md`, `tasks.md`).

## Detailed Findings

### Spec 001-extended-metadata-store — Extended Metadata Store

**Aligned ✓**
- FR-01 `put` 0–128 KiB — `src/lib.rs:158-170`
- FR-02 `get` clone / `NotFound` — `src/lib.rs:172-180`
- FR-03 `delete` idempotent — `src/lib.rs:182-191`
- FR-04 `iterate_all` snapshot — `src/lib.rs:193-199`
- FR-05 `force_flush` via installed `FlushTrigger`, blocks, maps errors to `StorageError`; no-op in in-memory mode — `src/lib.rs:201-215`, `attach_flush_trigger` `src/lib.rs:111`, `FlushTrigger` `src/lib.rs:68` (**verified: the 2026-08-20 backfill matches code**)
- FR-06 128 KiB `ValueTooLarge` — `src/lib.rs:159`, `MAX_VALUE_SIZE` `src/lib.rs:63`
- FR-07 dual-region ping-pong flush — `src/flush.rs:20-53`
- FR-08 recovery reads superblock, loads active region — `src/recovery.rs:23-53`
- FR-09 fallback to inactive region — `src/recovery.rs:63-78`
- FR-10 fresh partition auto-format — `src/recovery.rs:82-103`, `src/lib.rs:130-137`
- FR-12 coalesced flush — `src/flush.rs:142-165` (shared-wait on `flush_in_progress` condvar)
- FR-13 final flush on Drop — `src/flush.rs:187-191` (shutdown branch), `src/flush.rs:239-253` (Drop)
- FR-14 dirty count tracks mutations — `src/lib.rs:168,189`
- FR-15 `define_component!` provides `IExtendedMetadataStore` — `src/lib.rs:40-60`
- FR-16 optional `ILogger` receptacle — `src/lib.rs:44-45`, used at `:147,162,173,183,194,202`
- FR-17 persistence-wiring API (`initialize_from_client`, `snapshot_entries`, `mark_flushed`, `load_entries`, `dirty_count`, `flush_seq`) — `src/lib.rs:70-155`
- FR-18 `Superblock::region_capacity_bytes()` — `src/on_disk.rs:142-144`
- NFR-01 `RwLock<HashMap>` — `src/lib.rs:48`
- NFR-02 CRC32 on superblock/region/entry — `src/on_disk.rs:73,165,242,357-370`
- NFR-03 sector alignment — `src/on_disk.rs:346-351`
- NFR-04 crash consistency (ping-pong commit) — `src/flush.rs:40-52`
- NFR-05 `on_disk` always compiled, I/O modules `testing`-gated — `src/lib.rs:26-38`
- NFR-06 in-memory default build — `src/lib.rs:40-60` (no feature gate)
- NFR-07 `MockBlockDevice` fault injection + `read_write_stats` — `src/test_support.rs:130-145,223-225` (**verified RESOLVED**)
- NFR-08 `DmaAllocFn` abstraction — `src/block_io.rs:5,41,87`, `heap_dma_alloc` `src/test_support.rs:244`
- NFR-09 little-endian fields — `src/on_disk.rs:372-403`
- NFR-10 magic `0x4345_5254_4D45_5441` — `src/on_disk.rs:6`
- NFR-11 `create_test_component_from_state()` — `src/test_support.rs:272-278`
- SC-1 nine unit tests in `src/lib.rs` — verified 9 `#[test]` fns (`src/lib.rs:227-334`); crate now a workspace member (`Cargo.toml:23,105`) so SC-1/2/3/6/7 are exercisable (**verified RESOLVED**)

**Drifted ⚠️**

- **FR-11 / User Story 6 — dirty-count threshold trigger NOT implemented — moderate (NEW this sweep).**
  - Spec: FR-11 "background flush with timer **+ dirty threshold**"; US6 "supports configurable timer interval **and dirty-count threshold**".
  - Actual: `FlushConfig::dirty_threshold` is a public configurable field (default 100) at `src/flush.rs:61,68`, but the worker loop `src/flush.rs:172-208` **never reads it**. Flushes occur only on the timer interval (when `dirty_count > 0`) or on explicit `trigger_flush()`; `put()`/`delete()` do not signal the manager on crossing a threshold. The test `flush_manager_dirty_threshold_triggers` (`tests/persistence.rs:542-583`) passes via its 50 ms timer (sleeps 200 ms), not via threshold logic — its comment even says "Short interval to ensure timer fires". The threshold trigger is effectively absent and the config field is inert. Prior sweeps marked FR-11 "Implemented"; this is a missed drift.
  - Direction: ALIGN (implement the threshold; do not relax the spec) — `ALIGN-EMS-003`.

- **SSD integration test uses internal APIs for durability — moderate (carried forward).**
  - Requirement basis: previously `002-FR-011` (interface-only usage). That spec is now absent (see HUMAN_DECISION), but the code gap is real.
  - Actual: `tests/integration_ssd.rs` routes `put`/`get`/`delete`/`iterate_all` through the interface, but obtains durability and does startup via inherent/internal APIs (`initialize_from_client`, `snapshot_entries`, `flush::flush_to_disk`, `mark_flushed`, `load_entries`). Now that FR-05 is implemented, durability can move onto `IExtendedMetadataStore::force_flush()` with an installed trigger.
  - Direction: ALIGN — `ALIGN-EMS-001` (existing).

- **`CapacityExhausted` never surfaced to the caller — moderate (carried forward).**
  - Requirement basis: previously `002-FR-007`; the interface variant exists at `../interfaces/src/iextended_metadata_store.rs:12`.
  - Actual: `put()` enforces only the 128 KiB `ValueTooLarge` limit (`src/lib.rs:159`); region capacity is enforced only at flush time as a `String` error ("exceeds region capacity", `src/flush.rs:32-38`) and is never mapped to `ExtendedMetadataStoreError::CapacityExhausted`. `capacity_exhaustion_detected` (`tests/persistence.rs:627-660`) asserts on the flush-time `String`, never reaching the variant.
  - Direction: ALIGN — `ALIGN-EMS-002` (existing).

**Not Implemented ✗**
- None.

**Resolved since prior sweep (verified present in code) ✓**
- NFR-07 mock `read_write_stats` — `src/test_support.rs:223-225`.
- Workspace membership (SC-1/2/3/6/7) — `Cargo.toml:23,105`.
- FR-05 durability — `src/lib.rs:201-215` (backfilled 2026-08-20; re-verified).

## Sibling-Artifact Drift (spec-lag, fixed this sweep via BACKFILL)

- **`plan.md`** carried two stale claims contradicted by current code:
  - line ~104: "`force_flush()` ... is currently *not* wired ... an unconditional no-op" — FALSE since FR-05 (`src/lib.rs:201-215`). Backfilled.
  - lines ~160-162: "`extended-metadata-store` ... is currently **not** [a workspace member]" — FALSE (`Cargo.toml:23,105`). Backfilled.
- **`tasks.md`** listed ALIGN-001/ALIGN-002 as open MAJOR defects and T056 as "blocked on ALIGN-001"; both are RESOLVED. Backfilled to reflect resolution and to reference the current ALIGN-EMS-001/002/003 tasks.
- **`spec.md`** FR-11 row/status annotated to reflect the partial implementation (target requirement preserved); a new Known-Gaps entry added for the dirty-threshold gap.

## Unspecced Features

None new. The three items from the prior sweep are already documented in `spec.md`:
`Superblock::region_capacity_bytes()` (FR-18), `create_test_component_from_state()`
(NFR-11), and `CapacityExhausted` (Known Gaps + ALIGN-EMS-002).

## HUMAN_DECISION

- **Missing spec `002-ssd-integration-test`.** The 001 spec Known Gaps and the
  ALIGN tasks reference `002-FR-007` / `002-FR-011`, but no `specs/002-ssd-integration-test/`
  exists (absent from git HEAD; only backups remain). The SSD tests it described still
  exist (`tests/integration_ssd.rs`). Decide between: **(A)** restore the 002 spec from
  `.specify/sync/backups/specs/002-ssd-integration-test/spec.md.bak`, or **(B)** fold its
  requirements into 001 and renumber ALIGN-EMS-001/002. Recorded in
  `.specify/sync/align-tasks.md`; not resolved in this sweep.

## Recommendations
1. Implement the FR-11 dirty-threshold trigger (ALIGN-EMS-003) and tighten its test.
2. Resolve the 002-spec absence (HUMAN_DECISION) so the dangling `002-*` references resolve.
3. Move SSD-test durability onto `force_flush()` (ALIGN-EMS-001) and surface `CapacityExhausted` (ALIGN-EMS-002).
