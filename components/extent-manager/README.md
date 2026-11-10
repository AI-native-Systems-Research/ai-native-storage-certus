# extent-manager

Crash-consistent fixed-size extent allocator for the Certus storage system. Maps logical extent keys to physical disk locations using a region-sharded buddy+slab allocation scheme with dual-copy checkpoint persistence.

## Summary

`ExtentManager` implements the `IExtentManager` trait. It provides:

- Two-phase extent allocation (reserve, write data, then publish or abort)
- Region-sharded concurrency (power-of-two count, each with its own lock, buddy allocator, slab allocator, and per-slab key vectors)
- Crash-consistent checkpointing on a dedicated metadata device (dual-copy rotation with CRC32 protection)
- Checkpoint coalescing (at most two IO rounds regardless of concurrent callers)
- Background periodic checkpoint thread (configurable interval, default 30 seconds)
- Recovery from the most recent valid checkpoint on initialization

## Architecture

### On-Disk Layout

```
Metadata Device:
+----------+----------+------------------+------------------+
|Superblock| Padding  | Checkpoint Copy 0| Checkpoint Copy 1|
|  4 KiB   | (align)  |                  |                  |
+----------+----------+------------------+------------------+

Data Device:
+------------------+------------------+-----+------------------+
| Region 0 (buddy) | Region 1 (buddy) | ... | Region N (buddy) |
| slabs + extents  | slabs + extents  |     | slabs + extents  |
+------------------+------------------+-----+------------------+
```

**Superblock** (4 KiB, sector 0 of the metadata device): contains magic (`CERTUSV4`), format version (5), disk geometry parameters, active checkpoint copy indicator, checkpoint region offsets, instance ID, and a trailing CRC32. Written atomically on format and after each checkpoint.

**Checkpoint regions**: two equally-sized copies placed after the superblock (aligned to `metadata_alignment`). Each checkpoint contains a 16-byte header (sequence number, payload length, CRC32) followed by serialized region state (all slab descriptors and their per-slot key vectors). Writes alternate between copies; the superblock's `active_copy` field is flipped only after the new checkpoint is fully written, ensuring crash consistency.

**Data device**: divided into N power-of-two regions. Each region uses a buddy allocator for coarse slab allocation and a slab allocator for fine-grained slot management within each slab.

### Crash Consistency

Durability relies on the dual-copy checkpoint scheme. A crash during checkpoint write leaves the previous copy intact because the superblock (which selects the active copy) is only updated after the new checkpoint data is fully persisted. On recovery, the system reads the superblock, selects the active copy, validates its CRC, and rebuilds region state from the checkpoint payload.

### Interfaces

| Interface | Role | Description |
|-----------|------|-------------|
| `IExtentManager` | Provided | Two-phase extent allocation, checkpointing, recovery |
| `IBlockDevice` (metadata) | Receptacle | Metadata NVMe device for superblock and checkpoints |
| `ILogger` | Receptacle | Structured logging |

## Build

This crate is excluded from the workspace `default-members` and must be built explicitly.

```bash
cargo build -p extent-manager
```

### Features

| Feature | Description |
|---------|-------------|
| `spdk` (default) | Enable SPDK interface dependencies |
| `testing` | Expose `test_support` module with MockBlockDevice and helpers |
| `volatile_write_cache` | Enable NVMe flush after checkpoint writes (for drives with volatile write cache) |

## Test

Tests use an in-memory `MockBlockDevice` and heap-based DMA allocation (via the `testing` feature).

```bash
cargo test -p extent-manager
```

## Benchmarks

Criterion-based benchmarks covering reserve/publish, enumerate, remove, and checkpoint operations.

```bash
cargo bench -p extent-manager
```
