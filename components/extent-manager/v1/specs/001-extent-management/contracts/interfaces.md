# Interface Contracts: Extent Management

**Feature**: 001-extent-management
**Date**: 2026-04-16

## Overview

The extent manager exposes two interfaces via the component-framework. All public interaction occurs through these traits — no standalone public functions exist outside interface implementations.

## IExtentManager (Provider Interface)

Primary operational interface for extent lifecycle management.

### create_extent

Creates a new extent and persists its metadata to the block device.

**Parameters**:
- `key: u64` — Unique identifier for the extent (non-zero)
- `size_class: u32` — Extent size in bytes (must match a configured size)
- `filename: &str` — Optional filename (empty string = none, max 255 bytes)
- `data_crc: u32` — CRC-32 of extent data (only used if `has_crc` is true)
- `has_crc: bool` — Whether `data_crc` is valid

**Returns**: `Result<Vec<u8>, ExtentManagerError>` — Serialized extent metadata on success

**Errors**:
- `DuplicateKey` — An extent with this key already exists
- `InvalidSizeClass` — Requested size is not a configured size class
- `OutOfSpace { size_class }` — No free slots in the requested size class
- `IoError` — Block device write failed (propagated immediately)
- `NotInitialized` — Component not yet initialized or opened

**Atomicity**: Two-phase write. Record block written first, then bitmap block. Crash at any point is recoverable.

**Thread safety**: Acquires write lock. Blocks concurrent create/remove/iterate. Concurrent lookups may proceed before the lock is taken.

### remove_extent

Removes an extent by key, freeing its allocation slot.

**Parameters**:
- `key: u64` — Key of the extent to remove

**Returns**: `Result<(), ExtentManagerError>`

**Errors**:
- `KeyNotFound` — No extent with this key exists
- `IoError` — Block device write failed
- `NotInitialized` — Component not yet initialized or opened

**Atomicity**: Two-phase write. Bitmap bit cleared first, then record block zeroed. Crash at any point is recoverable.

**Thread safety**: Acquires write lock.

### lookup_extent

Looks up extent metadata by key.

**Parameters**:
- `key: u64` — Key of the extent to find

**Returns**: `Result<Vec<u8>, ExtentManagerError>` — Serialized extent metadata

**Errors**:
- `KeyNotFound` — No extent with this key exists
- `NotInitialized` — Component not yet initialized or opened

**Thread safety**: Acquires read lock. Can proceed concurrently with other lookups.

### extent_count

Returns the total number of allocated extents across all size classes.

**Returns**: `u64`

**Thread safety**: Acquires read lock.

## IExtentManagerAdmin (Provider Interface)

Administrative interface for lifecycle management (initialization, open, DMA setup).

### set_dma_alloc

Sets the DMA buffer allocation function. Must be called before initialize() or open().

**Parameters**:
- `alloc: DmaAllocFn` — Function that allocates 4KiB-aligned DMA buffers

### initialize

Formats a new volume: writes superblock, zeroes bitmap and record regions.

**Parameters**:
- `sizes: Vec<u32>` — Extent sizes in bytes (1–32 entries, each 128KiB–5MiB, multiples of 4KiB)
- `slots: Vec<u32>` — Slot counts per size class (1–10,000,000 each)
- `ns_id: u32` — NVMe namespace ID

**Returns**: `Result<(), NvmeBlockError>`

**Preconditions**: DMA allocator set, block device receptacle wired.

### open

Opens an existing volume: reads superblock, loads bitmaps, runs crash recovery.

**Parameters**:
- `ns_id: u32` — NVMe namespace ID

**Returns**: `Result<RecoveryResult, NvmeBlockError>`

**Recovery behavior**:
- Scans all slots: record CRC vs bitmap bit
- Orphan records (bitmap=0, valid CRC): zeroed
- Corrupt records (bitmap=1, invalid CRC): bitmap cleared, record zeroed
- Returns counts of extents loaded, orphans cleaned, and corrupt records found

## IBlockDevice (Receptacle)

Block device for persistent metadata storage. Channel-based actor model with `Command`/`Completion` enums carrying `Arc<DmaBuffer>`. Supports read and write at specified LBAs.

## ILogger (Receptacle)

Logger for diagnostic output. Provides `name(&self) -> &str` for component identification.

## Error Types

### ExtentManagerError

| Variant | Description |
|---------|-------------|
| `CorruptMetadata` | On-disk metadata failed integrity check |
| `DuplicateKey` | Attempted to create extent with existing key |
| `InvalidSizeClass` | Requested size not in configured size classes |
| `IoError` | Block device I/O operation failed |
| `KeyNotFound` | No extent exists for the given key |
| `NotInitialized` | Component used before initialize() or open() |
| `OutOfSpace { size_class: u32 }` | No free slots for the requested size class |

### RecoveryResult

| Field | Type | Description |
|-------|------|-------------|
| `extents_loaded` | u64 | Number of valid extents found on open |
| `orphans_cleaned` | u64 | Orphan records zeroed during recovery |
| `corrupt_records` | u64 | Corrupt records cleared during recovery |
