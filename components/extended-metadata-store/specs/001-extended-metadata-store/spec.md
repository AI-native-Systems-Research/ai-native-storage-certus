# Feature Specification: Extended Metadata Store

**Feature Branch**: `001-extended-metadata-store`
**Created**: 2026-07-08
**Status**: Backfilled
**Source**: Generated from existing implementation
**Last Synced**: 2026-09-03 — verification sweep confirmed this spec still matches the implementation: FR-05 trigger (`src/lib.rs:201-215`), `MockBlockDevice::read_write_stats` (`src/test_support.rs:223`), and workspace membership all still hold; default build (9 lib unit tests), `--features testing` (19 persistence tests), clippy, and doc are all green. No changes required. The one open item remains the `CapacityExhausted` ALIGN gap below (maintainer decision 2026-09-03: keep parked as documented). Prior sweep 2026-08-20 (Phase B): the previously-drafted FR-05 (`force_flush`) durable-flush-trigger fix is now **present and merged** in the working tree, so FR-05 is backfilled to **Implemented**. The two blockers the prior sweep recorded are **RESOLVED**: `MockBlockDevice` now implements `read_write_stats` (NFR-07), and the crate is now a member of the root `Cargo.toml` `[workspace] members`/`[workspace.dependencies]` (SC-1/2/3/6/7), so CI builds and tests it. Three previously-undocumented public items (`Superblock::region_capacity_bytes()`, `create_test_component_from_state()`, and the `CapacityExhausted` interface variant) are backfilled below. Interface-only test usage (002-FR-011) and surfacing capacity to the caller (002-FR-007) remain code-side gaps tracked as ALIGN tasks in `.specify/sync/align-tasks.md`.
> _Prior sync (2026-08-07, branch `sync/spec-drift-sweep-20260807`): FR-05 fix drafted on branch; dead public APIs documented; capacity-at-flush behavior clarified in the 002 spec; workspace-membership defect (SC 1/2/3/6/7) left deferred._

## Backfill Notice
> This spec was generated from existing code via `speckit.sync.backfill`.
> It documents current behavior, not original intent.
> Review carefully and update to reflect desired behavior.

## Overview

The Extended Metadata Store is a crash-consistent key-value storage component for the Certus storage system. It provides persistent metadata storage for arbitrary binary values keyed by string identifiers, with durability guarantees via a dual-region ping-pong flush strategy on NVMe block devices.

The component operates in two modes:
1. **In-memory mode** (default, no features): Pure in-memory HashMap store suitable for testing and non-persistent workloads.
2. **Persistent mode** (`testing`/`spdk` features): Full crash-consistent persistence via block device I/O, dual-region layout, CRC32-protected on-disk format, and background flush management.

Key design properties:
- Thread-safe concurrent access via `RwLock` (readers do not block readers)
- Crash consistency via atomic superblock commit point
- Automatic recovery from corruption via inactive region fallback
- Configurable background flush with dirty-count threshold and timer triggers
- Maximum value size: 128 KiB per entry
- Sector-aligned on-disk format (4096-byte default sector size)

## User Scenarios & Testing

### User Story 1 - Store and Retrieve Metadata (Priority: P1)

**As** a Certus filesystem component,
**I want** to store and retrieve arbitrary binary metadata by string key,
**so that** I can persist configuration, file attributes, and system state across operations.

**Acceptance Criteria:**
- `put(key, value)` stores the value; subsequent `get(key)` returns an identical copy
- Values from 0 bytes up to 128 KiB are supported
- Values exceeding 128 KiB return `ValueTooLarge` error
- Overwriting an existing key replaces the value atomically
- `get()` on a non-existent key returns `NotFound` error
- All operations are logged via the optional `ILogger` receptacle

**Tests:**
- `put_and_get` (unit): basic round-trip
- `get_not_found` (unit): missing key returns error
- `put_overwrites_existing` (unit): overwrite semantics
- `put_value_too_large` (unit): size limit enforcement
- `put_get_roundtrip_varied_sizes` (persistence): 0B, 1B, 4KiB, 128KiB values
- `test_put_get_small_value`, `test_put_get_medium_value`, `test_put_get_max_value` (SSD integration)

### User Story 2 - Persistence Across Restarts (Priority: P1)

**As** a system operator,
**I want** metadata to survive process restarts and crashes,
**so that** I do not lose critical filesystem state.

**Acceptance Criteria:**
- After `put` + `flush_to_disk`, data survives simulated reboot (new component from same disk state)
- Recovery reads the superblock, identifies the active region, deserializes entries
- If the active region is corrupt, recovery falls back to the inactive region
- If both regions are corrupt, the store initializes as empty (no crash)
- Fresh (all-zeros) partitions are detected and formatted automatically
- Flush sequence numbers are monotonically increasing

**Tests:**
- `flush_and_recover_from_reboot` (persistence): write + flush + reboot + verify
- `recovery_via_initialize_from_client` (persistence): full recovery path
- `recovery_fallback_to_inactive_region` (persistence): corruption fallback
- `recovery_fresh_partition_formats_empty` (persistence): virgin disk handling
- `crash_mid_flush_recovers_previous_state` (persistence): fault injection
- `test_persistence_after_flush`, `test_unflushed_entries_may_be_lost` (SSD integration)

### User Story 3 - Delete Metadata Entries (Priority: P2)

**As** a Certus filesystem component,
**I want** to delete metadata entries,
**so that** stale or obsolete metadata is removed and does not consume storage.

**Acceptance Criteria:**
- `delete(key)` removes the entry from in-memory state
- Deleted entries are excluded from `iterate_all()`
- After flush + reboot, deleted entries remain absent
- Deleting a non-existent key is idempotent (returns `Ok(())`)
- Delete increments the dirty count

**Tests:**
- `delete_existing_key` (unit): basic delete
- `delete_nonexistent_is_ok` (unit): idempotent delete
- `delete_persists_across_reboot` (persistence): durability of deletes
- `delete_nonexistent_is_idempotent` (persistence): repeated delete
- `test_delete_existing_key`, `test_delete_not_in_iterate` (SSD integration)

### User Story 4 - Iterate Over All Entries (Priority: P2)

**As** a Certus filesystem component,
**I want** to enumerate all stored metadata entries,
**so that** I can perform bulk operations like cache warming or state reconstruction.

**Acceptance Criteria:**
- `iterate_all()` returns a snapshot of all current key-value pairs
- The snapshot is consistent (no partial updates visible)
- Deleted entries are not included
- Concurrent writers do not corrupt the iteration result

**Tests:**
- `iterate_all_returns_all_entries` (unit): basic enumeration
- `iterate_all_returns_all_100_entries` (persistence): scale test
- `iterate_all_excludes_deleted` (persistence): delete interaction
- `concurrent_iterate_during_writes` (persistence): concurrent safety
- `test_iterate_all_complete`, `test_iterate_all_empty_store` (SSD integration)

### User Story 5 - Thread-Safe Concurrent Access (Priority: P2)

**As** a multi-threaded Certus server,
**I want** multiple threads to safely read and write metadata concurrently,
**so that** I do not need external synchronization.

**Acceptance Criteria:**
- Multiple concurrent `get()` calls do not block each other (shared read lock)
- `put()` and `delete()` acquire an exclusive write lock
- No panics under concurrent stress (8 threads, 1000 ops each)
- Final state after concurrent operations is self-consistent

**Tests:**
- `concurrent_stress_8_threads` (persistence): multi-threaded stress
- `concurrent_iterate_during_writes` (persistence): read-write concurrency

### User Story 6 - Force Flush and Background Flush (Priority: P3)

**As** a system that needs durability guarantees,
**I want** explicit flush control and automatic background flushing,
**so that** I can choose between latency and durability trade-offs.

**Acceptance Criteria:**
- `force_flush()` triggers an immediate flush and blocks until durable
- Multiple concurrent `force_flush()` calls coalesce into a single flush operation
- Background `FlushManager` supports configurable timer interval and dirty-count threshold
- `FlushManager` performs a final flush on shutdown (Drop)
- When no entries are dirty, `force_flush()` returns immediately (no-op)

**Tests:**
- `flush_manager_force_flush_persists` (persistence): explicit flush durability — exercises `FlushManager::trigger_flush()` directly, not `IExtendedMetadataStore::force_flush()`
- `flush_manager_dirty_threshold_triggers` (persistence): threshold-based flush
- `flush_manager_no_dirty_no_op` (persistence): no-op when clean
- `force_flush_succeeds` (unit): in-memory mode no-op

> **Note**: The first acceptance criterion above ("`force_flush()` triggers an immediate flush and blocks until durable") describes the contract of `IExtendedMetadataStore::force_flush()`, which is now **met** when the wiring layer installs a flush trigger via `attach_flush_trigger` (see FR-05 and its Known Gaps entry). In pure in-memory mode (no trigger installed) `force_flush()` is a correct no-op. The `flush_manager_*` tests listed below still drive `FlushManager::trigger_flush()` directly; a follow-up test that installs a trigger and asserts durability through `force_flush()` is noted under 002-FR-007's ALIGN task.

## Requirements

### Functional Requirements

| ID | Requirement | Status |
|----|-------------|--------|
| FR-01 | `put(key, value)` stores arbitrary binary data (0 to 128 KiB) by string key | Implemented |
| FR-02 | `get(key)` returns a clone of the stored value or `NotFound` error | Implemented |
| FR-03 | `delete(key)` removes entry from store; idempotent for missing keys | Implemented |
| FR-04 | `iterate_all()` returns a consistent snapshot of all entries | Implemented |
| FR-05 | `force_flush()` ensures all mutations are durable on disk: it invokes a durable-flush trigger installed by the wiring layer via `attach_flush_trigger` and blocks until it returns, mapping trigger errors to `StorageError`; in pure in-memory mode (no trigger installed) it is a correct no-op | Implemented (`src/lib.rs:201-215`; trigger install `attach_flush_trigger` at `src/lib.rs:111`, `FlushTrigger` alias at `src/lib.rs:68`) |
| FR-06 | Value size limit of 128 KiB enforced with `ValueTooLarge` error | Implemented |
| FR-07 | Dual-region ping-pong flush: writes to inactive region, then flips superblock | Implemented |
| FR-08 | Recovery reads superblock, loads active region entries into memory | Implemented |
| FR-09 | Recovery falls back to inactive region if active region is corrupt | Implemented |
| FR-10 | Fresh (unformatted) partitions are auto-detected and formatted | Implemented |
| FR-11 | `FlushManager` provides background flush with timer + dirty threshold | Implemented |
| FR-12 | Multiple `force_flush()` calls coalesce into single flush operation | Implemented |
| FR-13 | `FlushManager` performs final flush on Drop (graceful shutdown) | Implemented |
| FR-14 | Dirty count tracks mutations since last flush | Implemented |
| FR-15 | Component uses `define_component!` macro with `IExtendedMetadataStore` interface | Implemented |
| FR-16 | Optional `ILogger` receptacle for debug logging of operations | Implemented |
| FR-17 | External persistence-wiring API (`initialize_from_client`, `snapshot_entries`, `mark_flushed`, `load_entries`, `dirty_count()`, `flush_seq()` on `ExtendedMetadataStoreComponent`) provides the startup-recovery and flush-on-demand mechanism for persistent-mode deployments; a caller/dispatcher wires a `BlockDeviceClient` and drives these methods (directly or via `FlushManager`) to obtain durability | Implemented |
| FR-18 | `Superblock::region_capacity_bytes()` (`src/on_disk.rs`) is a public on-disk-format accessor returning the usable per-region byte capacity (`region_a_size * sector_size`), so callers/tests can compute capacity headroom without reconstructing the geometry. Part of the always-compiled on-disk-format API (see NFR-05) | Implemented |

### Known Gaps

- **FR-05 / `force_flush()` durability — RESOLVED (implemented).** Historically `force_flush()` was an unconditional no-op in every build configuration; the fix drafted on branch `sync/spec-drift-sweep-20260807` is now **present and merged** in the working tree (`src/lib.rs:201-215`). The component carries an optional type-erased `FlushTrigger` (`pub type FlushTrigger = Box<dyn Fn() -> Result<(), String> + Send + Sync>`, `src/lib.rs:68`), installed by the wiring layer through `attach_flush_trigger` (`src/lib.rs:111`, typically `move || flush_manager.trigger_flush()`). `force_flush()` invokes the trigger and blocks until it returns, mapping any error to `ExtendedMetadataStoreError::StorageError`; in pure in-memory mode (no trigger installed) it remains a correct no-op. The two verification blockers the prior sweep recorded are also **RESOLVED**: `MockBlockDevice` now implements `read_write_stats` (`src/test_support.rs:223`), so the `testing`-feature test build compiles, and the crate is now a member of the root workspace, so CI exercises it. Wiring callers may still drive the FR-17 API directly; both paths now yield durability.

- **`CapacityExhausted` not surfaced to the caller (open — tracked as ALIGN).** The interface defines `ExtendedMetadataStoreError::CapacityExhausted` (`../interfaces/src/iextended_metadata_store.rs:12`) but no code path constructs it: `put()` enforces only the 128 KiB per-value `ValueTooLarge` limit (`src/lib.rs:159`), while region/partition capacity is enforced at flush time inside `flush::flush_to_disk` as a `String` error ("exceeds region capacity", `src/flush.rs:32-38`). Consequently the 002 SSD test `test_capacity_exhaustion` never reaches the `CapacityExhausted` branch. Surfacing capacity to the caller is a code-side change tracked in `.specify/sync/align-tasks.md` (Task ALIGN-EMS-002 for 002-FR-007); until then the variant remains a defined-but-unconstructed part of the public interface contract.

- **Retained public API surface** (previously flagged as dead; now backfilled as documented requirements). `Superblock::region_capacity_bytes()` (`src/on_disk.rs:142`) is documented by FR-18. `create_test_component_from_state()` (`src/test_support.rs:272`, `testing`-gated) is documented by NFR-11. `StorageError` is now constructed by `force_flush()` trigger failures (FR-05). `CapacityExhausted` is covered by the ALIGN note above.

### Non-Functional Requirements

| ID | Requirement | Status |
|----|-------------|--------|
| NFR-01 | Thread-safe: `RwLock` allows concurrent readers, exclusive writers | Implemented |
| NFR-02 | On-disk format uses CRC32 integrity checks on superblock, region headers, and entry records | Implemented |
| NFR-03 | All on-disk structures are sector-aligned (4096 bytes default) | Implemented |
| NFR-04 | Crash consistency: partial flush never corrupts previously-committed data | Implemented |
| NFR-05 | Persistence I/O and runtime modules (`block_io`, `flush`, `recovery`, `test_support`) are gated behind `testing`/`spdk` feature flags; the on-disk format definitions module (`on_disk.rs`, defining `Superblock`/`RegionHeader`/`EntryRecord`) is intentionally always compiled since it contains pure data-format code with no I/O dependency, and downstream consumers need the format types available without enabling `testing`/`spdk` | Implemented |
| NFR-06 | In-memory mode works without SPDK (default compilation) | Implemented |
| NFR-07 | Test infrastructure provides `MockBlockDevice` with fault injection for deterministic testing | Implemented |
| NFR-08 | DMA memory allocation abstracted via `DmaAllocFn` for portability between test (heap) and production (hugepages) | Implemented |
| NFR-09 | Little-endian byte order for all on-disk integer fields | Implemented |
| NFR-10 | Superblock magic number `0x4345_5254_4D45_5441` ("CERTMETA") for format identification | Implemented |
| NFR-11 | Test infrastructure provides `create_test_component_from_state()` (`src/test_support.rs`, `testing`-gated): it reconstructs an `ExtendedMetadataStoreComponent` over a reboot-preserved `MockState` (via `MockBlockDevice::reboot_from`), so persistence tests can simulate a process restart against the same virtual disk contents | Implemented |

## Key Entities

### On-Disk Structures

| Entity | Description | Location |
|--------|-------------|----------|
| `Superblock` | 1-sector header: magic, version, sector size, partition geometry, active region pointer, flush sequence, entry count, CRC32 | `src/on_disk.rs` |
| `RegionHeader` | 1-sector header per region: flush sequence, entry count, total data bytes, CRC32 | `src/on_disk.rs` |
| `EntryRecord` | Variable-size record: key_len(2B) + value_len(4B) + flags(2B) + CRC32(4B) + key + value, padded to sector alignment | `src/on_disk.rs` |

### Runtime Structures

| Entity | Description | Location |
|--------|-------------|----------|
| `ExtendedMetadataStoreComponent` | Main component: `RwLock<HashMap>` store, `AtomicU64` dirty count and flush sequence | `src/lib.rs` |
| `BlockDeviceClient` | Partition-aware I/O wrapper over `IBlockDevice` client channels with DMA allocation | `src/block_io.rs` |
| `FlushManager` | Background flush thread with configurable timer/threshold, coalesced flush support | `src/flush.rs` |
| `FlushConfig` | Configuration: interval (default 5s), dirty threshold (default 100) | `src/flush.rs` |
| `RecoveryResult` | Recovery output: superblock, recovered entries, warnings | `src/recovery.rs` |

### Test Infrastructure

| Entity | Description | Location |
|--------|-------------|----------|
| `MockBlockDevice` | In-memory block device with `HashMap<u64, Vec<u8>>` storage, SPSC channel worker thread | `src/test_support.rs` |
| `MockState` | Shared state for `MockBlockDevice` enabling reboot simulation | `src/test_support.rs` |
| `FaultConfig` | Fault injection: `fail_after_n_writes` for crash simulation | `src/test_support.rs` |

## Dependencies

### Required (Always)
- `component-framework` (workspace): Component model infrastructure
- `component-core` (workspace): Core traits (`IUnknown`, `query_interface!`, channels)
- `component-macros` (workspace): `define_component!`, `define_interface!` proc macros
- `interfaces` (workspace): `IExtendedMetadataStore`, `ILogger`, `IBlockDevice` trait definitions

### Optional (Feature-Gated)
- `block-device-spdk-nvme` (optional, `spdk` feature): Real NVMe block device driver
- `disk-partition-manager` (optional, `spdk` feature): Partition table management
- `spdk-env` (optional, `spdk` feature): SPDK environment initialization
- `logger` (optional, `spdk` feature): Logger component for integration tests

### Interface Contracts
- **Provides**: `IExtendedMetadataStore` (put, get, delete, iterate_all, force_flush)
- **Inherent API** (`ExtendedMetadataStoreComponent`, FR-17): `initialize_from_client`, `snapshot_entries`, `mark_flushed`, `load_entries`, `dirty_count()`, `flush_seq()` — the persistence-wiring contract used by callers/dispatchers to drive recovery-on-startup and flush-on-demand
- **Receptacles**: `ILogger` (optional, for debug logging)

## Success Criteria

1. All 9 unit tests in `src/lib.rs` pass (in-memory mode, no features)
2. All persistence tests in `tests/persistence.rs` pass with `--features testing`
3. All SSD integration tests in `tests/integration_ssd.rs` pass with `--features spdk` on NVMe hardware
4. Data survives simulated crashes (fault injection) with recovery to last-good state
5. No panics under 8-thread concurrent stress (1000 ops/thread)
6. `cargo clippy -- -D warnings` passes clean
7. All public items have doc comments; `cargo doc --no-deps` is warning-free

> **Note (RESOLVED 2026-08-20)**: Success Criteria 1, 2, 3, 6, and 7 are now exercisable by the standard build/test/CI path. The crate is a member of the root `Cargo.toml` `[workspace] members` (and `[workspace.dependencies]`), and `MockBlockDevice` implements the current `IBlockDevice` trait (including `read_write_stats`), so the default and `testing`-feature builds compile and CI runs them. The prior build-wiring defect (previously tracked as ALIGN-001) is closed.

## Implementation Notes

### On-Disk Layout

```
Partition Layout:
+------------------+-------------------+-------------------+
| Superblock (1s)  | Region A (N sec)  | Region B (N sec)  |
+------------------+-------------------+-------------------+

Region Layout:
+------------------+----------+----------+-----+----------+
| RegionHeader(1s) | Entry 0  | Entry 1  | ... | Entry N  |
+------------------+----------+----------+-----+----------+

Entry Layout (sector-padded):
+--------+----------+-------+-------+-----+-------+
| key_len| value_len| flags | CRC32 | key | value |
| 2B     | 4B       | 2B    | 4B    |     |       |
+--------+----------+-------+-------+-----+-------+
```

### Flush Strategy (Ping-Pong)

1. Serialize all in-memory entries to a byte buffer
2. Write buffer to the **inactive** region (the one not currently pointed to by superblock)
3. Update superblock: flip `active_region`, increment `flush_seq`, update `entry_count`
4. Write superblock to LBA 0 (the atomic commit point)

This ensures that a crash at any point leaves either the old or new data intact:
- Crash during step 2: old active region is still valid
- Crash during step 4: old superblock still points to previous valid region

### Recovery Algorithm

1. Read superblock from LBA 0
2. If no valid superblock (bad magic/CRC): format fresh partition
3. If `flush_seq == 0`: empty store (formatted but never flushed)
4. Read active region, verify header CRC and entry CRCs
5. If active region corrupt: try inactive region (best-effort recovery)
6. If both corrupt: format fresh (data loss, logged as warning)

### Feature Gating

- `testing` feature: enables `block_io`, `flush`, `recovery`, `test_support` modules and activates `interfaces/spdk`
- `spdk` feature: implies `testing`, adds runtime dependencies (`block-device-spdk-nvme`, `disk-partition-manager`, `spdk-env`, `logger`)
- Default (no features): pure in-memory store with `force_flush()` as a no-op (intended and correct in this mode — no trigger is installed and there is no disk to flush to)
- Under `testing`/`spdk`: `IExtendedMetadataStore::force_flush()` performs a durable flush when the wiring layer has installed a `FlushTrigger` via `attach_flush_trigger` (typically `move || flush_manager.trigger_flush()`), blocking until it completes and mapping errors to `StorageError` (see FR-05). Callers may equivalently drive the FR-17 wiring API (`flush::flush_to_disk()` / `FlushManager::trigger_flush()`) directly; both paths obtain durability.
