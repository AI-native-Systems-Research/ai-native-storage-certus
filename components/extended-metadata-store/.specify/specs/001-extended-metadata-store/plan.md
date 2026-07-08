# Implementation Plan: Extended Metadata Store

**Branch**: `001-extended-metadata-store` | **Date**: 2026-07-08 | **Spec**: [spec.md](spec.md)
**Context**: Backfilled from existing implementation. Documents current architecture.

## Summary

The Extended Metadata Store is a crash-consistent key-value metadata storage component for Certus. It provides an in-memory `RwLock<HashMap>` with optional persistence via a dual-region (A/B) ping-pong layout on NVMe block devices. The component operates in three modes: pure in-memory (no features), mock-persistence (feature `testing`), and real-hardware (feature `spdk`). A background `FlushManager` with condvar-based signaling handles periodic and on-demand flushes, while a recovery module rebuilds state from disk on startup.

## Technical Context

**Language/Version**: Rust stable, edition 2021, MSRV 1.75
**Primary Dependencies**:
- `component-framework` (workspace) -- `define_component!` macro, framework traits
- `component-core` (workspace) -- `IUnknown`, `query_interface!`, SPSC channels, binding
- `component-macros` (workspace) -- `define_interface!` proc macro
- `interfaces` (workspace) -- `IExtendedMetadataStore`, `ILogger`, `IBlockDevice`, `DmaBuffer`, `DmaAllocFn`
- `block-device-spdk-nvme` (optional, feature `spdk`) -- Real NVMe I/O driver
- `disk-partition-manager` (optional, feature `spdk`) -- Partition table management
- `spdk-env` (optional, feature `spdk`) -- SPDK environment init, hugepage memory
- `logger` (optional, feature `spdk`) -- Console/file logger bound to `ILogger` receptacle

## Architecture

### Component Layer

```
+------------------------------------------------------------------+
|                       certus-server / consumer                    |
+------------------------------------------------------------------+
            |                                          |
            | IExtendedMetadataStore                   | ILogger (optional)
            v                                          v
+---------------------------+               +-------------------+
| ExtendedMetadataStore     |<-- receptacle-| LoggerComponent   |
| Component                 |               +-------------------+
|                           |
| +-- RwLock<HashMap> ------+----> in-memory key-value store
| +-- AtomicU64 dirty_count |
| +-- AtomicU64 flush_seq   |
+---------------------------+
            |
            | (feature "testing"/"spdk")
            v
+---------------------------+         +---------------------+
| FlushManager              |-------->| BlockDeviceClient   |
| (background worker thread)|         | (partition-aware    |
| - condvar timer + signal  |         |  sector-aligned I/O)|
| - dirty threshold trigger |         +---------------------+
| - coalescing force_flush  |                    |
+---------------------------+                    | Command/Completion
                                                 | (SPSC channels)
                                                 v
                                      +---------------------+
                                      | IBlockDevice impl   |
                                      | (MockBlockDevice or |
                                      |  BlockDeviceSpdkNvme|
                                      +---------------------+
                                                 |
                                                 v
                                           [ NVMe SSD ]
```

### Internal Module Structure

```
components/extended-metadata-store/
  Cargo.toml
  src/
    lib.rs              -- Component definition, IExtendedMetadataStore impl, unit tests
    on_disk.rs          -- Superblock, RegionHeader, EntryRecord structs; serialize/deserialize;
                           CRC32 (IEEE 0xEDB88320); sector-alignment helpers
    block_io.rs         -- BlockDeviceClient: partition-relative LBA I/O over IBlockDevice channels
                           (feature "testing")
    flush.rs            -- flush_to_disk() function; FlushManager background thread with
                           FlushConfig (interval, dirty_threshold) (feature "testing")
    recovery.rs         -- recover_from_disk(), format_partition(), format_fresh();
                           dual-region fallback logic (feature "testing")
    test_support.rs     -- MockBlockDevice (shared-state, fault injection, reboot simulation),
                           heap_dma_alloc(), test component factories (feature "testing")
  tests/
    persistence.rs      -- MockBlockDevice-based persistence, recovery, delete, iterate,
                           concurrency, FlushManager, capacity, crash-mid-flush tests
                           (feature "testing")
    integration_ssd.rs  -- Real NVMe hardware tests via SPDK; partition setup,
                           put/get/delete/iterate/persistence/capacity (feature "spdk")
```

### Data Flow

**Write path (put)**:
1. Caller invokes `store.put(key, value)` via `IExtendedMetadataStore`.
2. Value size is validated against `MAX_VALUE_SIZE` (128 KiB).
3. `RwLock` write lock acquired; entry inserted into HashMap; lock released.
4. `dirty_count` atomically incremented.
5. Optional: `ILogger` debug log emitted.

**Flush path (periodic or on-demand)**:
1. `FlushManager` worker wakes on timer expiry, dirty-threshold exceeded, or explicit `trigger_flush` signal.
2. `snapshot_entries()` clones the HashMap under read lock (point-in-time snapshot).
3. `flush_to_disk()` serializes entries via `serialize_region()` into sector-aligned bytes.
4. Region data written to the **inactive** region (the one not pointed to by the superblock).
5. Superblock updated: active-region pointer flipped, flush_seq incremented, entry_count set.
6. Superblock written to LBA 0 (single-sector atomic commit point).
7. `dirty_count` reset to 0; `flush_seq` updated in component.

**Recovery path (startup)**:
1. `recover_from_disk()` reads superblock from LBA 0.
2. If no valid superblock: return empty (caller formats fresh via `format_partition`).
3. If `flush_seq == 0`: store was formatted but never flushed; return empty entries.
4. Read active region; deserialize entries with CRC32 validation.
5. If active region corrupt (CRC mismatch or seq mismatch): fall back to inactive region.
6. If both corrupt: format fresh.
7. `load_entries()` replaces in-memory HashMap with recovered data.

### Key Design Decisions

| Decision | Rationale |
|----------|-----------|
| Dual-region (A/B) ping-pong | Guarantees at least one valid checkpoint survives any crash; the inactive region is never partially overwritten during normal operation. |
| Single-sector superblock as commit point | A single 4 KiB sector write is atomic on NVMe; flipping the active-region pointer in one write provides atomic commit semantics. |
| `RwLock<HashMap>` (not lock-free) | Simplicity; multiple concurrent readers allowed; write path is short (insert + atomic increment). Adequate for metadata workloads. |
| Custom CRC32 (IEEE polynomial) | Zero external dependencies for integrity checking; well-understood algorithm; compact implementation. |
| Feature-gated persistence modules | Allows the component to be used as a pure in-memory store in unit tests of upstream consumers, without pulling in SPDK or mock infrastructure. |
| Sector-aligned entry padding | Required for O_DIRECT and SPDK DMA buffers; avoids sub-sector read-modify-write overhead. |
| FlushManager with condvar (not busy-spin) | Worker thread sleeps efficiently; multiple `trigger_flush` callers coalesce to one in-flight flush. |
| MockBlockDevice with shared-state reboots | Enables testing the full persistence/recovery cycle without hardware; fault injection for crash simulation. |
| Little-endian serialization | Cross-platform compatibility; x86-native for zero-cost on primary target. |

## Dependencies

| Module | Depends On | Nature |
|--------|-----------|--------|
| `lib.rs` | `component-framework`, `interfaces` | Core framework + interface trait |
| `on_disk.rs` | (none external) | Pure data structures + serialization |
| `block_io.rs` | `interfaces::iblock_device` (ClientChannels, Command, Completion, DmaAllocFn) | I/O abstraction |
| `flush.rs` | `block_io`, `on_disk` | Orchestrates persistence |
| `recovery.rs` | `block_io`, `on_disk` | Reads and validates disk state |
| `test_support.rs` | `interfaces::iblock_device`, `component_core::channel` | Test infrastructure |
| integration tests | `block-device-spdk-nvme`, `disk-partition-manager`, `spdk-env`, `logger` | Real hardware path |

## Testing

| Level | Location | Feature Gate | Coverage |
|-------|----------|--------------|----------|
| Unit tests | `src/lib.rs` (mod tests) | none | put/get/delete/iterate/force_flush/dirty_count/value_too_large |
| Unit tests | `src/on_disk.rs` (mod tests) | none | Superblock round-trip, entry round-trip, region round-trip, CRC corruption, padding |
| Persistence tests | `tests/persistence.rs` | `testing` | Full I/O path via MockBlockDevice: flush, multi-flush alternation, reboot recovery, fallback to inactive, delete persistence, iterate consistency, 8-thread concurrency stress, FlushManager force/threshold/no-op, capacity exhaustion, crash-mid-flush |
| Integration tests | `tests/integration_ssd.rs` | `spdk` | Real NVMe: put/get varied sizes, delete, iterate, persistence across restart, unflushed data not served, bulk integrity (500 entries), capacity exhaustion |

## Future Considerations

- **Compression**: The `flags` field (u16, currently 0) in `EntryRecord` is reserved for future use such as per-entry compression or tombstone markers.
- **Incremental/WAL-based flush**: Current design serializes the entire HashMap on every flush. For large stores, a write-ahead log or delta-based approach would reduce flush I/O.
- **Batch/transaction API**: No multi-key atomic operations are supported; adding a batch-put or transaction envelope may be needed for complex metadata updates.
- **Capacity feedback to callers**: Currently `flush_to_disk` returns a capacity error, but the in-memory `put()` has no pre-flight capacity check. A `CapacityExhausted` error at put time would require tracking serialized size.
- **Asynchronous I/O**: The block_io layer uses synchronous sector-at-a-time reads/writes. Batching multiple sectors per NVMe command (via async submission) would improve flush throughput.
- **External CRC crate**: The custom CRC32 implementation is correct but could be replaced with `crc32fast` for SIMD-accelerated checksums on large values.
- **Key size limit**: Keys are unbounded UTF-8 strings (up to u16::MAX bytes in the on-disk format). An explicit key-length limit at the API level would provide clearer contracts.
- **Metrics/telemetry**: No operational metrics (flush latency, dirty-count histograms, recovery time) are exposed; integrating with a telemetry receptacle would aid production monitoring.
