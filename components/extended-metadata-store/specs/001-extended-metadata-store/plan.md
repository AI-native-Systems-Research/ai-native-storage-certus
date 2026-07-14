# Implementation Plan: Extended Metadata Store

**Branch**: `001-extended-metadata-store` | **Date**: 2026-07-08 | **Spec**: [spec.md](spec.md)
**Context**: Backfilled from existing implementation.

## Summary

The Extended Metadata Store provides a crash-consistent key-value storage component for the Certus filesystem. It implements the `IExtendedMetadataStore` interface with operations: `put`, `get`, `delete`, `iterate_all`, and `force_flush`. Data is persisted to NVMe block devices via a dual-region ping-pong layout that guarantees crash consistency through an atomic superblock commit.

The implementation is layered:
1. **On-disk format** (`on_disk.rs`): Superblock, RegionHeader, EntryRecord with CRC32 protection
2. **Block I/O** (`block_io.rs`): Partition-aware sector-aligned read/write via `IBlockDevice` channels
3. **Flush logic** (`flush.rs`): Dual-region flush + `FlushManager` background thread
4. **Recovery** (`recovery.rs`): Superblock-guided region deserialization with corruption fallback
5. **Core component** (`lib.rs`): In-memory HashMap with `RwLock`, dirty tracking, interface implementation
6. **Test infrastructure** (`test_support.rs`): `MockBlockDevice` with fault injection and reboot simulation

## Technical Context

### Component Framework Integration

The component is defined using `define_component!` macro from the Certus component framework:
- Provides: `IExtendedMetadataStore`
- Receptacles: `ILogger` (optional)
- Fields: `store: RwLock<HashMap<String, Vec<u8>>>`, `dirty_count: AtomicU64`, `flush_seq: AtomicU64`

### Concurrency Model

- `RwLock<HashMap>` allows concurrent readers with exclusive writers
- `AtomicU64` for lock-free dirty count and flush sequence tracking
- `FlushManager` runs a dedicated background thread with `Condvar`-based wake/sleep
- Flush coalescing: multiple `trigger_flush()` callers share a single in-flight flush via `Condvar` wait

### On-Disk Format Design Decisions

- **Sector alignment**: All structures padded to 4096-byte boundaries for atomic NVMe writes
- **CRC32**: Custom implementation (polynomial 0xEDB88320) without external dependencies
- **Little-endian**: All multi-byte integers stored in LE byte order
- **Magic number**: `0x4345_5254_4D45_5441` ("CERTMETA") for format identification
- **Format version**: 1 (allows future incompatible changes with clean rejection)
- **Dual regions**: A and B split the usable space after the superblock sector equally

### Feature Flag Architecture

```
default (no features) -> pure in-memory store
  |
  +-- testing -> enables block_io, flush, recovery, test_support
  |               activates interfaces/spdk for IBlockDevice types
  |
  +-- spdk -> testing + runtime SPDK dependencies
                enables integration tests on real NVMe hardware
```

## Architecture

### Module Dependency Graph

```
lib.rs (component definition + IExtendedMetadataStore impl)
  |
  +-- on_disk.rs (Superblock, RegionHeader, EntryRecord, serialize/deserialize)
  |     [always compiled]
  |
  +-- block_io.rs (BlockDeviceClient - sector I/O via IBlockDevice channels)
  |     [cfg(feature = "testing")]
  |
  +-- flush.rs (flush_to_disk + FlushManager background thread)
  |     [cfg(feature = "testing")]
  |     depends on: block_io, on_disk
  |
  +-- recovery.rs (recover_from_disk, format_fresh, format_partition)
  |     [cfg(feature = "testing")]
  |     depends on: block_io, on_disk
  |
  +-- test_support.rs (MockBlockDevice, heap_dma_alloc, FaultConfig)
        [cfg(feature = "testing")]
        depends on: interfaces::iblock_device
```

### Data Flow: Put + Flush

```
Application -> put(key, value)
  -> validate size <= 128 KiB
  -> acquire write lock on HashMap
  -> insert entry
  -> release lock
  -> increment dirty_count (atomic)

FlushManager (background) OR force_flush() (explicit):
  -> snapshot_entries() [acquire read lock, clone HashMap, release]
  -> serialize_region(entries, flush_seq, sector_size)
  -> write to inactive region via BlockDeviceClient
  -> update Superblock (flip active, bump seq, update count)
  -> write Superblock to LBA 0 (ATOMIC COMMIT POINT)
  -> mark_flushed(new_seq) [reset dirty_count]
```

### Data Flow: Recovery

```
initialize_from_client(client, total_sectors):
  -> read_superblock() from LBA 0
  -> if no valid superblock: format_partition(), return empty
  -> if flush_seq == 0: return empty (formatted, never flushed)
  -> try active region: read + deserialize
  -> if valid: load_entries(entries), return
  -> if corrupt: try inactive region
  -> if both corrupt: format_fresh(), return empty with warnings
```

## Dependencies

### Internal (Workspace)

| Crate | Role | Required |
|-------|------|----------|
| `component-framework` | `define_component!` macro facade | Always |
| `component-core` | `IUnknown`, `query_interface!`, SPSC channels | Always |
| `component-macros` | Proc macros for component/interface definitions | Always |
| `interfaces` | `IExtendedMetadataStore`, `ILogger`, `IBlockDevice`, `DmaAllocFn`, `DmaBuffer` | Always |
| `block-device-spdk-nvme` | Real NVMe driver (SPDK-based) | `spdk` feature |
| `disk-partition-manager` | Partition table management | `spdk` feature |
| `spdk-env` | SPDK environment initialization + hardware detection | `spdk` feature |
| `logger` | Logger component for integration tests | `spdk` feature |

### External

None. The component has zero external crate dependencies. CRC32 is implemented inline.

## Testing

### Test Layers

1. **Unit tests** (`src/lib.rs`, `src/on_disk.rs`): Pure in-memory, no features required
   - 8 tests in `lib.rs`: put/get, not_found, delete, overwrite, iterate, flush, size limit, dirty count
   - 5 tests in `on_disk.rs`: superblock round-trip, corruption detection, entry round-trip, region round-trip, padding

2. **Persistence tests** (`tests/persistence.rs`, `--features testing`): MockBlockDevice-based
   - 20+ tests covering: flush/verify, region alternation, reboot recovery, corruption fallback, delete persistence, iterate correctness, concurrent stress, FlushManager, capacity exhaustion, crash mid-flush

3. **SSD integration tests** (`tests/integration_ssd.rs`, `--features spdk`): Real NVMe hardware
   - 12 tests: put/get varied sizes, overwrite, delete, persistence after flush, iterate, bulk integrity, capacity

### Test Infrastructure

- `MockBlockDevice`: In-memory block device with `HashMap<u64, Vec<u8>>` sector storage
- `MockState`: Shared state enabling reboot simulation (same data, new channels)
- `FaultConfig.fail_after_n_writes`: Deterministic crash injection for testing partial-flush recovery
- `heap_dma_alloc()`: Heap-backed DMA allocator for testing without SPDK hugepages
- `create_test_component()` / `create_test_component_from_state()`: Wiring helpers

### Running Tests

```bash
# Unit tests (no features, always available)
cargo test -p extended-metadata-store

# Persistence tests (MockBlockDevice, no hardware needed)
cargo test -p extended-metadata-store --features testing

# SSD integration tests (requires SPDK + NVMe hardware)
cargo test -p extended-metadata-store --features spdk
```

## Future Considerations

1. **Capacity checking on put()**: Currently capacity exhaustion is only detected at flush time. The in-memory put succeeds even if the store would exceed on-disk capacity. A pre-flight capacity check could reject puts earlier.

2. **Incremental flush**: Currently all entries are serialized on every flush (full snapshot). For large stores, delta/incremental flush could reduce write amplification.

3. **Compaction**: Deleted entries consume no space (HashMap-based), but a journal-based approach would need compaction.

4. **Encryption at rest**: Entry values could be encrypted before serialization for security-sensitive metadata.

5. **Batch operations**: `put_batch()` and `delete_batch()` for atomic multi-key operations could reduce flush overhead.

6. **Configurable sector size**: Currently hardcoded to 4096; some NVMe devices use 512-byte sectors.

7. **Metrics export**: Expose dirty count, flush latency, recovery time via a telemetry interface.

8. **WAL (Write-Ahead Log)**: For workloads needing per-operation durability without full-store flush, a WAL could provide entry-level persistence.
