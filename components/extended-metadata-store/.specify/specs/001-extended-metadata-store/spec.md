# Feature Specification: Extended Metadata Store

**Feature Branch**: `001-extended-metadata-store`
**Created**: 2026-07-08
**Status**: Backfilled
**Source**: Generated from existing implementation

## Backfill Notice

> This spec was generated from existing code via `speckit.sync.backfill`.
> It documents current behavior, not original intent.
> Review carefully and update to reflect desired behavior.

## Overview

The Extended Metadata Store is a key-value storage component for the Certus storage system that provides persistent, crash-consistent metadata storage. It implements the `IExtendedMetadataStore` interface, offering put/get/delete/iterate operations with values up to 128 KiB. The in-memory HashMap is protected by a `RwLock` for concurrent access and periodically flushed to an NVMe block device via a dual-region ping-pong layout.

The on-disk format uses a superblock (1 sector) followed by two equal-sized regions (A and B). Flushes serialize all entries to the currently inactive region, then atomically flip the superblock pointer. This ensures that a crash at any point during a flush leaves the previous valid checkpoint intact on the other region. Recovery reads the superblock, loads the active region, and falls back to the inactive region if corruption is detected.

## User Scenarios & Testing

### User Story 1 - Put and Retrieve Metadata (Priority: P1)

As a Certus system component, I want to store and retrieve arbitrary metadata by string key, so that extended file attributes, caching hints, and object-level metadata can be persisted alongside data extents.

**Acceptance Scenarios**:
- Given an empty store, when I put key "k1" with value "v1", then get("k1") returns "v1".
- Given a key exists with value "v1", when I put the same key with value "v2", then get returns "v2" (last-writer-wins).
- Given a key does not exist, when I get it, then `ExtendedMetadataStoreError::NotFound` is returned.
- Given a value of exactly 128 KiB, when I put it, then it succeeds.
- Given a value of 128 KiB + 1 byte, when I put it, then `ExtendedMetadataStoreError::ValueTooLarge` is returned.
- Given a zero-length value, when I put it, then it succeeds and get returns an empty byte vector.

### User Story 2 - Delete Metadata (Priority: P1)

As a Certus system component, I want to delete metadata entries by key, so that stale or expired metadata does not consume storage.

**Acceptance Scenarios**:
- Given a key exists, when I delete it, then get returns `NotFound`.
- Given a key does not exist, when I delete it, then the operation returns `Ok(())` (idempotent).
- Given a key was deleted, when I call iterate_all, then the deleted key is not present.

### User Story 3 - Crash-Consistent Persistence (Priority: P1)

As a storage system operator, I want metadata to survive process restarts and power failures (after flush), so that I do not lose important file metadata on crash.

**Acceptance Scenarios**:
- Given entries are written and flush_to_disk completes, when the process restarts and recovery runs, then all flushed entries are present with correct values.
- Given entries are written but NOT flushed, when the process restarts, then those entries are NOT present (no corruption guarantee).
- Given a crash occurs mid-flush (partial region write), when recovery runs, then the store recovers from the previous valid checkpoint without serving corrupt data.
- Given the active region is corrupt (CRC mismatch), when recovery runs, then it falls back to the inactive region and logs a warning.
- Given both regions are corrupt and no valid superblock exists, when recovery runs, then the partition is formatted fresh as an empty store.

### User Story 4 - Enumerate All Entries (Priority: P2)

As a Certus system component, I want to iterate over all stored key-value pairs, so that I can perform bulk operations like migration, replication, or garbage collection.

**Acceptance Scenarios**:
- Given N entries in the store, when iterate_all is called, then exactly N entries are returned with correct keys and values.
- Given an empty store, when iterate_all is called, then an empty vector is returned.
- Given concurrent writes are happening, when iterate_all is called, then a consistent point-in-time snapshot is returned (no partial entries, no duplicates).

### User Story 5 - Background and On-Demand Flush (Priority: P1)

As a storage system operator, I want both periodic background flushes and on-demand force_flush, so that I can control the durability/performance tradeoff.

**Acceptance Scenarios**:
- Given the FlushManager is running with a dirty threshold of N, when N mutations accumulate, then a flush is triggered automatically.
- Given the FlushManager is running, when force_flush (trigger_flush) is called, then it blocks until the flush completes and data is durable.
- Given no dirty entries exist, when force_flush is called, then it returns immediately (no-op).
- Given the FlushManager is dropped, then a final flush is performed before the worker thread exits.

### User Story 6 - Concurrent Access (Priority: P1)

As a multi-threaded Certus server, I want multiple threads to safely read and write metadata concurrently, so that the store does not corrupt under parallel access.

**Acceptance Scenarios**:
- Given 8 threads performing 1000 mixed put/get/delete operations each, then no panics occur and final state is self-consistent.
- Given concurrent iterate_all and write operations, then iterate returns a valid snapshot with no duplicate keys.

## Requirements

### Functional Requirements

- **FR-001**: The store MUST implement the `IExtendedMetadataStore` interface providing `put`, `get`, `delete`, `iterate_all`, and `force_flush` methods.
- **FR-002**: Keys are UTF-8 strings; values are arbitrary byte vectors up to 128 KiB (`MAX_VALUE_SIZE`).
- **FR-003**: `put` with an existing key MUST overwrite the previous value (last-writer-wins, no versioning).
- **FR-004**: `delete` of a nonexistent key MUST succeed (idempotent delete).
- **FR-005**: `iterate_all` MUST return a point-in-time snapshot (held under read lock) with no duplicates.
- **FR-006**: The store MUST track a dirty count that increments on every `put` and `delete` operation.
- **FR-007**: The store MUST support loading entries from disk via `load_entries` and snapshotting entries via `snapshot_entries` for the flush/recovery subsystem.
- **FR-008**: The on-disk format MUST use a dual-region (A/B) ping-pong layout with CRC32 integrity checks on the superblock, region headers, and individual entry records.
- **FR-009**: Flush MUST write entries to the inactive region, then atomically flip the superblock active-region pointer (single-sector write as the commit point).
- **FR-010**: Recovery MUST attempt the active region first; if corrupt, fall back to the inactive region; if both corrupt, format a fresh empty store.
- **FR-011**: The `FlushManager` MUST support both timer-based periodic flushes and dirty-count threshold triggers.
- **FR-012**: The `FlushManager` MUST perform a final flush on drop (graceful shutdown).
- **FR-013**: `force_flush` via `FlushManager::trigger_flush` MUST block until the flush is complete (coalescing multiple concurrent callers).

### Non-Functional Requirements

- **NFR-001**: All operations MUST be thread-safe. The in-memory store uses `RwLock<HashMap>` allowing concurrent readers.
- **NFR-002**: On-disk entries are sector-aligned (default 4096 bytes) to support O_DIRECT / SPDK DMA requirements.
- **NFR-003**: CRC32 checksums MUST be verified on deserialization; corrupt records are skipped gracefully without panicking.
- **NFR-004**: The component MUST operate in pure in-memory mode (no persistence) when the `testing`/`spdk` features are not enabled, with `force_flush` as a no-op.
- **NFR-005**: The component MUST log operations to an optional `ILogger` receptacle when bound (debug level).
- **NFR-006**: Maximum value size enforcement (128 KiB) MUST be checked before any allocation or write.
- **NFR-007**: The block I/O layer MUST support partition-relative LBA addressing via a configurable `base_lba` offset.
- **NFR-008**: Serialization format MUST be little-endian for cross-platform compatibility.
- **NFR-009**: The `FlushManager` worker thread MUST not busy-spin; it uses condvar-based wait with timeout.

## Key Entities

| Entity | Description |
|--------|-------------|
| `ExtendedMetadataStoreComponent` | Main component struct; holds `RwLock<HashMap<String, Vec<u8>>>`, `dirty_count`, `flush_seq`. Defined via `define_component!`. |
| `IExtendedMetadataStore` | Public interface trait with `put`, `get`, `delete`, `iterate_all`, `force_flush`. |
| `Superblock` | On-disk header (1 sector): magic, version, sector size, partition layout, active region pointer, flush sequence, entry count, CRC32. |
| `RegionHeader` | Per-region header (1 sector): flush sequence, entry count, total data bytes, CRC32. |
| `EntryRecord` | Single key-value record on disk: key_len (2B), value_len (4B), flags (2B), CRC32 (4B), key bytes, value bytes. Sector-padded. |
| `BlockDeviceClient` | Partition-aware I/O wrapper over `IBlockDevice` client channels. Translates partition-relative LBAs to absolute LBAs. |
| `FlushManager` | Background flush thread with timer + dirty-threshold triggers, condvar-based signaling, and graceful shutdown. |
| `FlushConfig` | Configuration: flush interval (default 5s), dirty threshold (default 100). |
| `RecoveryResult` | Output of `recover_from_disk`: superblock state, recovered entries, warning messages. |
| `MockBlockDevice` | Test double: in-memory block device with fault injection support for simulating crashes. |

## Dependencies

| Dependency | Type | Purpose |
|-----------|------|---------|
| `component-framework` | Workspace crate | `define_component!` macro and framework traits |
| `component-core` | Workspace crate | `IUnknown`, `query_interface!`, channels, binding |
| `component-macros` | Workspace crate | `define_interface!` for `IExtendedMetadataStore` |
| `interfaces` | Workspace crate | Shared interface definitions (`IExtendedMetadataStore`, `ILogger`, `IBlockDevice`, `DmaBuffer`) |
| `block-device-spdk-nvme` | Optional (feature `spdk`) | Real NVMe block device driver for production I/O |
| `disk-partition-manager` | Optional (feature `spdk`) | Partition table management for SSD layout |
| `spdk-env` | Optional (feature `spdk`) | SPDK environment initialization and hugepage memory |
| `logger` | Optional (feature `spdk`) | Console/file logger component bound to `ILogger` receptacle |

## Success Criteria

1. All unit tests pass (`cargo test -p extended-metadata-store`).
2. All persistence tests pass with the `testing` feature (`cargo test -p extended-metadata-store --features testing`).
3. Integration tests on real NVMe hardware pass with the `spdk` feature (when hardware is available).
4. No data corruption is observable after simulated crashes (fault injection tests pass).
5. 8-thread concurrent stress test completes without panics or inconsistent state.
6. `cargo clippy -- -D warnings` produces no warnings.
7. `cargo doc --no-deps -p extended-metadata-store` produces no warnings.

## Implementation Notes

- The component version is 0.1.0, indicating early development.
- The `testing` feature gates the block_io, flush, recovery, and test_support modules. Without it, the store operates as a pure in-memory HashMap with no persistence (useful for unit testing upstream components).
- The `spdk` feature implies `testing` and additionally brings in real hardware dependencies.
- The on-disk format uses a custom CRC32 implementation (IEEE polynomial 0xEDB88320) rather than an external crate.
- Entry records use a 12-byte header: `key_len` (u16, max 65535), `value_len` (u32, max ~4 GiB theoretical but limited to 128 KiB by `MAX_VALUE_SIZE`), `flags` (u16, currently always 0, reserved for future use such as compression or tombstone markers), and `crc32` (u32).
- The dual-region strategy means the store always has at least one valid checkpoint as long as at least one successful flush has completed. The worst case on crash is losing mutations since the last flush.
- The `FlushManager` uses a single worker thread with condvar-based wakeup. Multiple `trigger_flush` callers coalesce to a single in-flight flush operation.
- The `MockBlockDevice` supports fault injection (`fail_after_n_writes`) for testing crash scenarios, and shared-state "reboot" semantics for testing recovery across simulated restarts.
- Partition layout in integration tests: index 0 = internal metadata (CERTUS_METADATA), index 1 = extended metadata (CERTUS_EXTERNAL_META), each 128 MiB.
