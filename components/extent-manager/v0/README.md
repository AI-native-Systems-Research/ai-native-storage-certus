# Extent Manager v1

Fixed-size storage extent manager for the Certus storage system. Manages extent allocation, metadata persistence, and crash recovery on NVMe SSDs using 4KiB-atomic writes.

## Requirements

This crate requires the `spdk` feature (enabled by default) and a running SPDK environment with an `IBlockDevice` provider (e.g., `block-device-spdk-nvme`).

## Component Lifecycle

`ExtentManagerComponentV1` is a `define_component!` component that provides `IExtentManager` and requires `IBlockDevice` and `ILogger` via receptacles.

```text
1. Create:    ExtentManagerComponentV1::new_default()
2. Wire:      connect IBlockDevice to block_device, ILogger to logger
3. Init:      comp.initialize(sizes, slots, ns_id)  — or —  comp.open(ns_id)
4. Use:       call IExtentManager methods (create_extent, remove_extent, etc.)
```

## Architecture

- **Component model**: `define_component!` with `IBlockDevice` + `ILogger` receptacles, `IExtentManager` provider
- **Debug logging**: Emits `[extent-manager]` prefixed messages to stderr via `ILogger` receptacle
- **Bitmap allocation**: One bit per extent slot per size class
- **On-disk layout**: Superblock (block 0) + bitmap region + extent record region, all 4KiB-aligned
- **Crash consistency**: Two-phase atomic writes exploiting NVMe power-fail guarantees
- **Recovery**: Orphan detection and cleanup on startup via bitmap/record scan
- **Block device**: Uses `IBlockDevice` channel-based actor model via `BlockDevice` wrapper

## IExtentManager API

| Operation | Description |
|-----------|-------------|
| `create_extent` | Allocate slot, write record, flip bitmap bit, return serialized metadata |
| `remove_extent` | Clear bitmap bit (single atomic write) |
| `lookup_extent` | Read from in-memory HashMap index, return serialized metadata |
| `extent_count` | Return total extent count |

## Build & Test

```bash
cargo build -p extent-manager
cargo test -p extent-manager             # All tests (unit + integration + doc)
cargo test -p extent-manager test_name   # Run a single test by name
cargo doc -p extent-manager --no-deps    # Build documentation
cargo bench -p extent-manager            # Run Criterion benchmarks
cargo bench -p extent-manager --no-run   # Verify benchmarks compile
```

### Test Suites

| Suite | Location | Count | Description |
|-------|----------|-------|-------------|
| Unit tests | `src/*.rs` | 44 | Bitmap, metadata, superblock, error, CRC |
| API operations | `tests/api_operations.rs` | 11 | Full IExtentManager CRUD via mock block device |
| Crash recovery | `tests/crash_recovery.rs` | 5 | Power-failure simulation with fault injection |
| Thread safety | `tests/thread_safety.rs` | 4 | Concurrent creates, removes, lookups |
| Doc tests | `src/*.rs` | 6 | Runnable examples in doc comments |

### Benchmarks

| Benchmark | File | Description |
|-----------|------|-------------|
| `create_extent` | `benches/create_benchmark.rs` | Throughput of extent creation |
| `lookup_extent` | `benches/lookup_benchmark.rs` | Lookup latency (1000 pre-populated extents) |
| `remove_extent` | `benches/remove_benchmark.rs` | Remove throughput (100K pre-populated) |
| `extent_count` | `benches/count_benchmark.rs` | Count latency at varying populations |

### CI Gate

```bash
cargo fmt -p extent-manager --check \
  && cargo clippy -p extent-manager -- -D warnings \
  && cargo test -p extent-manager \
  && cargo doc -p extent-manager --no-deps \
  && cargo bench -p extent-manager --no-run
```

Note: The extent-manager is excluded from the workspace `default-members` because it requires the SPDK feature and linked SPDK native libraries. Build it explicitly with `-p extent-manager`.

## Module Layout

| Module | Purpose |
|--------|---------|
| `lib.rs` | `ExtentManagerComponentV1` component with `define_component!`, `IExtentManager` impl |
| `error.rs` | `ExtentManagerError` enum (re-exported from interfaces crate) |
| `metadata.rs` | `ExtentMetadata`, `OnDiskExtentRecord`, CRC-32 |
| `block_device.rs` | `BlockDevice` wrapper bridging `IBlockDevice` to sync 4KiB block I/O |
| `superblock.rs` | On-disk superblock layout and validation |
| `bitmap.rs` | Per-size-class allocation bitmap |
| `recovery.rs` | Crash recovery: orphan detection and cleanup |
| `test_support.rs` | Mock block device, heap DMA allocator, fault injection (test/bench only) |
