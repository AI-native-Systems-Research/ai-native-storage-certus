# extended-metadata-store

**Crate**: `extended-metadata-store`
**Path**: `components/extended-metadata-store/`
**Version**: 0.1.0
**Features**: `testing` (persistence subsystem), `spdk` (real NVMe integration)

## Description

Crash-consistent key-value metadata store for the Certus storage system. Stores arbitrary byte blobs (up to 128 KiB per value) keyed by string. Uses an in-memory HashMap as the hot path and persists to an NVMe block device using a dual-region ping-pong layout for atomic commits.

## Component Definition

```
ExtendedMetadataStoreComponent {
    version: "0.1.0",
    provides: [IExtendedMetadataStore],
    receptacles: {
        logger: ILogger,
    },
    fields: {
        store: RwLock<HashMap<String, Vec<u8>>>,
        dirty_count: AtomicU64,
        flush_seq: AtomicU64,
    },
}
```

## Interface Definition

```rust
define_interface! {
    pub IExtendedMetadataStore {
        fn put(&self, key: &str, value: &[u8]) -> Result<(), ExtendedMetadataStoreError>;
        fn get(&self, key: &str) -> Result<Vec<u8>, ExtendedMetadataStoreError>;
        fn delete(&self, key: &str) -> Result<(), ExtendedMetadataStoreError>;
        fn iterate_all(&self) -> Result<Vec<(String, Vec<u8>)>, ExtendedMetadataStoreError>;
        fn force_flush(&self) -> Result<(), ExtendedMetadataStoreError>;
    }
}
```

## Receptacles

| Name | Interface | Required | Purpose |
|------|-----------|----------|---------|
| `logger` | `ILogger` | No | Optional logging for put/get/delete/recovery operations |

## Key Semantics

- **Dual-region crash-consistent layout**: Superblock (1 sector) + Region A + Region B. Writes to inactive region, then flips superblock pointer as atomic commit.
- **CRC32-protected**: all on-disk structures (superblock, region headers, entry records).
- **Background flush**: configurable interval (default 5s) and dirty-threshold (default 100 mutations). Concurrent callers coalesce.
- **Recovery**: reads superblock, loads active region; falls back to inactive on corruption; formats fresh if both corrupt.
- **Feature gating**: without `testing`/`spdk` features, operates as pure in-memory store where `force_flush` is a no-op.

## Key Types

- `ExtendedMetadataStoreError` — `NotFound`, `StorageError(String)`, `CapacityExhausted`, `ValueTooLarge`
