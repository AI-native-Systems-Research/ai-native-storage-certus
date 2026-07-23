# disk-partition-manager

**Crate**: `disk-partition-manager`
**Path**: `components/disk-partition-manager/`
**Version**: 0.1.0

## Description

GUID Partition Table (GPT) manager for Certus. Reads, validates, and writes GPT partition tables on NVMe devices via the `IBlockDevice` channel interface. Enables Certus to carve raw NVMe namespaces into named partitions (metadata, data, extended-metadata) with type GUIDs.

## Component Definition

```
DiskPartitionManagerComponent {
    version: "0.1.0",
    provides: [IPartitionTable],
    receptacles: {
        block_device: IBlockDevice,
    },
}
```

## Interface Definition

```rust
define_interface! {
    pub IPartitionTable {
        fn initialize(&self) -> Result<PartitionTable, PartitionTableError>;
        fn format(&self, config: PartitionConfig) -> Result<PartitionTable, PartitionTableError>;
        fn partition_info(&self, index: u32) -> Result<PartitionInfo, PartitionTableError>;
        fn num_partitions(&self) -> Result<u32, PartitionTableError>;
    }
}
```

## Receptacles

| Name | Interface | Required | Purpose |
|------|-----------|----------|---------|
| `block_device` | `IBlockDevice` | Yes | Sector-level reads/writes for GPT operations |

## Key Semantics

- **UEFI GPT compliance**: protective MBR (type 0xEE), primary + backup headers, 128 entries at 128 bytes each, CRC32 validation, v4 random GUIDs.
- **Fallback resilience**: `initialize()` tries primary header; on CRC failure falls back to backup at last LBA.
- **`initialize_or_format()` helper**: attempts initialize first; on `NoPartitionTable`/`CorruptTable` (or `force_format`), calls format. Returns `(PartitionTable, bool)` where bool indicates fresh format.
- **Flexible layout**: `PartitionConfig` supports explicit `size_bytes` per partition plus one "use remaining space" partition (`size_bytes == 0`).
- **Certus type GUIDs**: `CERTUS_METADATA`, `CERTUS_DATA`, `CERTUS_EXTERNAL_META`.
- **Cached state**: after init/format, the `PartitionTable` is cached in a `Mutex<Option<PartitionTable>>`.
- **Synchronous I/O**: uses `Command::ReadSync`/`WriteSync` over the block device channel.
