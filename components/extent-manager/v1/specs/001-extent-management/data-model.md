# Data Model: Extent Management

**Feature**: 001-extent-management
**Date**: 2026-04-16

## Entities

### Extent

The primary managed entity. Represents a fixed-size contiguous region of data on disk.

| Field | Type | Constraints | Description |
|-------|------|-------------|-------------|
| key | u64 | Unique, non-zero | Unique identifier provided by client |
| size_class | u32 | Must match a configured size (128KiB–5MiB, multiple of 4KiB) | Extent size in bytes |
| namespace_id | u32 | Set at init/open time | NVMe namespace ID |
| offset | u64 | Calculated at allocation | On-disk offset in 4KiB blocks |
| filename | Option&lt;String&gt; | Max 255 bytes UTF-8 | Optional filename associated with extent |
| data_crc | Option&lt;u32&gt; | CRC-32 of extent data | Optional integrity check for stored data |

**Identity**: Unique by `key` (u64). No two extents may share the same key.

**Lifecycle**: Created → Active → Removed

### Size Class

A configured extent size with a pool of allocation slots.

| Field | Type | Constraints | Description |
|-------|------|-------------|-------------|
| size | u32 | 128KiB–5MiB, multiple of 4KiB | Extent size in bytes |
| total_slots | u32 | 1–10,000,000 | Number of pre-provisioned allocation positions |
| allocated_count | u32 | 0–total_slots | Current number of allocated extents |
| bitmap_start_lba | u64 | Calculated at init | First LBA of bitmap blocks for this class |
| record_start_lba | u64 | Calculated at init | First LBA of extent record blocks for this class |

**Identity**: Unique by `size` value within a single extent manager instance.

**Relationship**: Contains 0..total_slots Allocation Slots. Each Allocation Slot holds 0 or 1 Extent.

### Superblock

On-disk management header at LBA 0.

| Field | Type | Constraints | Description |
|-------|------|-------------|-------------|
| magic | u64 | Must equal `0x4558544D475256_31` ("EXTMGRV1") | Format identifier |
| version | u32 | Must equal 1 | On-disk format version |
| num_size_classes | u32 | 1–32 | Number of configured size classes |
| sizes | [u32] | Length = num_size_classes | Array of extent sizes in bytes |
| slots | [u32] | Length = num_size_classes | Array of slot counts per size class |
| namespace_id | u32 | Provided at init | NVMe namespace for this volume |
| crc | u32 | CRC-32 of all preceding fields | Integrity check |

**Identity**: Singleton — exactly one per formatted block device.

### On-Disk Extent Record

Persistent per-slot metadata block (4KiB).

| Field | Offset | Size | Description |
|-------|--------|------|-------------|
| key | 0 | 8 bytes | Extent key (0 = empty slot) |
| size_class | 8 | 4 bytes | Extent size in bytes |
| namespace_id | 12 | 4 bytes | NVMe namespace ID |
| offset | 16 | 8 bytes | On-disk offset in 4KiB blocks |
| has_crc | 24 | 1 byte | 1 if data_crc is valid, 0 otherwise |
| data_crc | 25 | 4 bytes | CRC-32 of extent data (valid if has_crc=1) |
| filename_len | 29 | 2 bytes | Length of filename in bytes (0 = no filename) |
| filename | 31 | 0–255 bytes | UTF-8 filename bytes |
| (padding) | 286 | 3806 bytes | Zero-filled |
| record_crc | 4092 | 4 bytes | CRC-32 of bytes 0–4091 |

**Total**: 4096 bytes (one block per slot).

### Allocation Bitmap

Per-size-class bitfield tracking slot allocation state.

| Field | Type | Description |
|-------|------|-------------|
| bits | Vec&lt;u64&gt; | One bit per slot; 1=allocated, 0=free |
| num_slots | u32 | Total slots in this size class |

**On-disk**: Stored as ceil(num_slots / (4096 * 8)) consecutive 4KiB blocks. One block holds 32,768 bits. For 10M slots: 306 blocks.

**Relationship**: Each bit position corresponds to an Allocation Slot index within the size class.

## State Transitions

### Extent Lifecycle

```
[Empty Slot] --create_extent()--> [Active Extent] --remove_extent()--> [Empty Slot]
```

### Allocation Slot State

```
Free (bitmap=0, record=zeroed)
  │
  ├─ create phase 1: write record block ─→ Orphan (bitmap=0, record=valid)
  │
  ├─ create phase 2: set bitmap bit ─────→ Allocated (bitmap=1, record=valid)
  │
  └─ (crash during create: recovery zeroes orphan record)

Allocated (bitmap=1, record=valid)
  │
  ├─ remove phase 1: clear bitmap bit ──→ Pending-Free (bitmap=0, record=stale)
  │
  ├─ remove phase 2: zero record block ─→ Free (bitmap=0, record=zeroed)
  │
  └─ (crash during remove: recovery finds bitmap=0, zeroes stale record)
```

### Recovery Decision Matrix

| Bitmap | Record CRC | Action |
|--------|-----------|--------|
| 1 | Valid | Keep — extent is allocated and intact |
| 1 | Invalid | Clear bitmap bit, zero record — corrupt metadata |
| 0 | Valid | Zero record — orphan from interrupted create |
| 0 | Invalid/Zero | No action — slot is free |

## Validation Rules

- Key must be non-zero and unique across all size classes
- Size class must match one of the configured sizes
- Filename must be valid UTF-8, max 255 bytes
- CRC-32 must match computed value for superblock and extent records
- Block device I/O errors propagate immediately to caller (no retry)
- Bitmap and record updates must use 4KiB-aligned atomic writes
