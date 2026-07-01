# Implementation Plan: GPT Partition Management

**Branch**: `001-gpt-partition-management` | **Date**: 2026-07-01 | **Spec**: [spec.md](spec.md)
**Context**: Backfilled from existing implementation. Documents current architecture.

## Summary

The disk-partition-manager component provides GPT partition table read/write capability over NVMe block devices. It is used by the dispatcher components to divide each physical SSD into isolated regions (metadata, extended-metadata, data) before initializing the extent-manager.

## Technical Context

**Language/Version**: Rust stable, edition 2021, MSRV 1.75
**Primary Dependencies**:
- `interfaces` (with `spdk` feature) — `IBlockDevice`, `IPartitionTable`, partition types
- `component-framework` — `define_component!` macro
- `crc32fast` — CRC32 validation per UEFI GPT spec

**Performance Goals**: Format/initialize in constant time relative to device size (only header + entry sectors touched).

## Architecture

### Component Layer

```
DiskPartitionManager (define_component!)
├── receptacle: block_device (IBlockDevice)
├── provides: IPartitionTable
├── state: Mutex<Option<PartitionTable>>
└── ns_id: Mutex<Option<u32>>
```

### Internal Module Structure

```
src/
├── lib.rs       — Component definition, IPartitionTable impl, initialize_or_format()
└── gpt.rs       — GptManager: low-level GPT read/write, layout computation, I/O
```

### Data Flow

**Format path:**
1. Caller provides `PartitionConfig` (sector size, total sectors, partition specs)
2. `GptManager::write_gpt()` computes layout (fixed-size + rest-of-disk)
3. Writes: protective MBR (LBA 0), primary header (LBA 1), primary entries (LBA 2+), backup entries, backup header (last LBA)
4. Returns `PartitionTable` with resolved offsets

**Initialize path:**
1. `GptManager::read_gpt()` reads primary header at LBA 1
2. Validates header CRC32, then reads and validates entry array CRC32
3. On primary failure, falls back to backup header at last LBA
4. Parses non-empty entries into `PartitionInfo` vec

### Key Design Decisions

1. **Synchronous I/O via client channels**: Uses `Command::ReadSync`/`WriteSync` over the block device's SPSC channel rather than async I/O. This simplifies the initialization path (runs once at startup) at the cost of blocking the caller thread.

2. **Primary + backup GPT redundancy**: Standard UEFI practice. Protects against partial write failures during format (power loss after primary write but before backup).

3. **Rest-of-disk semantics**: Exactly one partition may specify `size_bytes=0`, meaning it absorbs all remaining space. This avoids requiring callers to know exact device capacity when specifying the data partition.

4. **Separate from extent-manager**: Partition management is its own component (not embedded in extent-manager) so multiple components can query partition offsets independently.

## Disk Layout

```
LBA 0:                  Protective MBR
LBA 1:                  Primary GPT Header (92 bytes, CRC32-protected)
LBA 2..N:              Primary Partition Entry Array (128 × 128 bytes = 16 KiB)
LBA N+1..M:            Partition 0 (metadata)
LBA M+1..K:            Partition 1 (extended-metadata)
LBA K+1..L:            Partition 2 (data, rest of disk)
LBA L+1..last-2:       Backup Partition Entry Array
LBA last:              Backup GPT Header
```

## Dependencies

- **IBlockDevice** (interfaces crate): Sector read/write via SPDK NVMe driver
- **Component Framework**: define_component!, receptacle binding

## Testing

No dedicated test files currently exist. Validation is performed at integration level via the dispatcher's `initialize_or_format()` call against real NVMe hardware.

**Recommended future tests:**
- Unit tests with mock block device (round-trip format → initialize)
- Corrupt primary header → backup fallback
- Layout error cases (overflow, multiple rest-of-disk)
- UTF-16LE name encoding edge cases

## Future Considerations

- Add `resize()` support for online partition growth
- Support GPT attribute flags (e.g., read-only, hidden)
- Add partition table repair (rebuild primary from backup or vice versa)
- Consider async I/O path for non-blocking initialization
