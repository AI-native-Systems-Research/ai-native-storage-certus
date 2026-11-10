# extent-manager (v2)

**Crate**: `extent-manager`
**Path**: `components/extent-manager/`
**Version**: 0.3.0

## Description

Fixed-size extent allocator with crash-consistent on-disk layout. Manages a data disk and a separate metadata disk. Supports two-phase extent reservation (allocate slot, get `WriteHandle`, publish or abort), lookup, removal, iteration, and periodic checkpointing for crash recovery.

### On-Disk Layout

- **Metadata device**: superblock + two checkpoint regions (for atomic swap)
- **Data device**: buddy allocator + slab allocator per region

### Crash Recovery

On `initialize`, reads the superblock and latest valid checkpoint from the metadata device, then reconstructs in-memory allocator state. Checkpoints are coalesced and can be triggered explicitly or by a background thread (default: every 30 seconds).

## Component Definition

```
ExtentManager {
    version: "0.3.0",
    provides: [IExtentManager],
    receptacles: {
        metadata_device: IBlockDevice,
        logger: ILogger,
    },
}
```

## Interface Definition

```rust
define_interface! {
    pub IExtentManager {
        fn format(&self, params: FormatParams) -> Result<(), ExtentManagerError>;
        fn initialize(&self) -> Result<(), ExtentManagerError>;
        fn reserve_extent(&self, key: ExtentKey, size: u32) -> Result<WriteHandle, ExtentManagerError>;
        fn get_extents(&self) -> Vec<Extent>;
        fn for_each_extent(&self, cb: &mut dyn FnMut(&Extent));
        fn remove_extent(&self, offset: u64) -> Result<(), ExtentManagerError>;
        fn checkpoint(&self) -> Result<(), ExtentManagerError>;
        fn get_instance_id(&self) -> Result<u64, ExtentManagerError>;
        fn set_checkpoint_interval(&self, interval: Option<std::time::Duration>);
        fn used_bytes(&self) -> u64;
        fn capacity_bytes(&self) -> u64;
        fn set_metadata_base_lba(&self, base_lba: u64);
        fn set_data_base_lba(&self, base_lba: u64);
        fn data_base_lba(&self) -> u64;
    }
}
```

## Verified Properties

The following invariants are formally proved with Creusot (see `components/extent-manager/verif/`):

| ID | Name | Description |
|----|------|-------------|
| P1 | sector-size-nonzero | `format` rejects sector_size == 0 |
| P2 | slab-alignment | `format` rejects slab_size not aligned to sector_size |
| P3 | extent-size-bounded | `format` rejects max_extent_size > slab_size |
| P4 | region-count-nonzero | `format` rejects region_count == 0 |
| P5 | size-aligned | `reserve_extent` aligns size up to sector_size boundary |
| P6 | publish-exactly-once | `WriteHandle::publish` consumes the handle |
| P7 | abort-on-drop | dropping uncommitted WriteHandle auto-aborts reservation |
| P8 | out-of-space | `reserve_extent` returns OutOfSpace when no capacity |
| P9 | offset-not-found | `remove_extent` returns OffsetNotFound for invalid offset |
| P10 | lifecycle-valid | reserve→publish produces Extent with aligned size and correct offset |

Total: 10 properties, 22 verification conditions discharged by SMT solvers.

## Receptacles

| Name | Interface | Required | Purpose |
|------|-----------|----------|---------|
| `metadata_device` | `IBlockDevice` | Yes | NVMe device for reading/writing superblock and checkpoints |
| `logger` | `ILogger` | No | Optional logging |

## Key Types

- `ExtentKey = u64`
- `Extent { key, size, offset }`
- `FormatParams { data_disk_size, slab_size, max_extent_size, sector_size, region_count, metadata_alignment, instance_id, metadata_disk_ns_id, metadata_region_size }`
- `WriteHandle` — two-phase commit handle: `publish()` commits, `abort()` or drop cancels
- `ExtentManagerError` — `CorruptMetadata`, `IoError`, `NotInitialized`, `OffsetNotFound`, `OutOfSpace`
