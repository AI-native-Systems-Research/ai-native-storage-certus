# dispatch-map (v0)

**Crate**: `dispatch-map`
**Path**: `components/dispatch-map/`
**Version**: 0.2.0

## Description

In-memory dispatch map that tracks where each extent's data currently lives — either in the DRAM memory-tier pool or at a committed byte offset on the block device (SSD). Implements readers-writer reference counting with blocking (`Condvar`) semantics and 2-second timeout. Delegates eviction ordering to an `IEvictionPolicy` receptacle.

On `initialize`, recovers all committed extents from the bound `IExtentManager` into the map (if extent_manager is connected).

## Component Definition

```
DispatchMapComponent {
    version: "0.2.0",
    provides: [IDispatchMap],
    receptacles: {
        logger: ILogger,
        extent_manager: IExtentManager,
        eviction_policy: IEvictionPolicy,
    },
}
```

## Interface Definition

```rust
define_interface! {
    pub IDispatchMap {
        fn initialize(&self) -> Result<(), DispatchMapError>;
        fn lookup(&self, key: CacheKey) -> Result<LookupResult, DispatchMapError>;
        fn convert_to_storage(&self, key: CacheKey, offset: u64) -> Result<(), DispatchMapError>;
        fn take_read(&self, key: CacheKey) -> Result<(), DispatchMapError>;
        fn take_write(&self, key: CacheKey) -> Result<(), DispatchMapError>;
        fn release_read(&self, key: CacheKey) -> Result<(), DispatchMapError>;
        fn release_write(&self, key: CacheKey) -> Result<(), DispatchMapError>;
        fn downgrade_reference(&self, key: CacheKey) -> Result<(), DispatchMapError>;
        fn remove(&self, key: CacheKey) -> Result<(), DispatchMapError>;
        fn touch(&self, key: CacheKey) -> Result<(), DispatchMapError>;
        fn entry_size(&self, key: CacheKey) -> Result<u32, DispatchMapError>;
        fn oldest_keys(&self, n: usize) -> Vec<CacheKey>;
        fn create_memory_tier_entry(&self, key: CacheKey, pointer: *mut u8, size: u32) -> Result<(), DispatchMapError>;
        fn convert_memory_tier_to_block(&self, key: CacheKey) -> Result<(), DispatchMapError>;
        fn promote_block_to_memory_tier(&self, key: CacheKey, pointer: *mut u8, size: u32) -> Result<(), DispatchMapError>;
        fn is_evictable(&self, key: CacheKey) -> bool;
        fn try_evict_to_block(&self, key: CacheKey) -> Result<(), DispatchMapError>;
        fn recover_extent(&self, key: CacheKey, offset: u64, size_blocks: u32) -> Result<(), DispatchMapError>;
    }
}
```

## Verified Properties

The following invariants are formally proved with Creusot (see `components/dispatch-map/verif/`):

| ID | Name | Description |
|----|------|-------------|
| P1 | read-underflow | `release_read` fails when read_ref == 0 |
| P2 | write-underflow | `release_write` fails when write_ref == 0 |
| P3 | write-binary | write_ref is always 0 or 1 (`take_write` sets exactly 1) |
| P4 | downgrade-requires-write | `downgrade_reference` fails without active write ref |
| P5 | downgrade-conservation | downgrade preserves total ref count (write+read constant) |
| P6 | remove-zero-refs | `remove` fails if any read or write references are active |
| P7 | create-no-duplicates | `create_memory_tier_entry` rejects existing keys |
| P8 | size-nonzero | `create_memory_tier_entry` rejects size == 0 |
| P9 | lookup-increments-read | successful lookup increments read_ref by exactly 1 |
| P10 | convert-requires-ssd-offset | `convert_memory_tier_to_block` requires ssd_offset present |

Total: 10 properties, 24 verification conditions discharged by SMT solvers.

## Receptacles

| Name | Interface | Required | Purpose |
|------|-----------|----------|---------|
| `logger` | `ILogger` | No | Optional logging |
| `extent_manager` | `IExtentManager` | No | Source of committed extents for recovery; starts empty if unbound |
| `eviction_policy` | `IEvictionPolicy` | Yes | LRU ordering for eviction decisions |

## Key Types

- `CacheKey = u64`
- `LookupResult` — `NotExist`, `MismatchSize`, `BlockDevice { offset: u64 }`, `MemoryTier { pointer: *mut u8, size: u32 }`
- `DispatchMapError` — `KeyNotFound`, `AlreadyExists`, `ActiveReferences`, `Timeout`, `AllocationFailed`, `InvalidSize`, `NotInitialized`, `RefCountUnderflow`, `RefCountOverflow`, `NoWriteReference`, `InvalidState`

## Key Design Decisions

- **Two-tier location model**: `Location::BlockDevice { offset }` or `Location::MemoryTier { pointer, size, ssd_offset }`. The `ssd_offset` field tracks whether write-through to SSD completed (prerequisite for eviction).
- **Atomic eviction**: `try_evict_to_block` checks evictability and transitions under single lock hold (no TOCTOU).
- **Entry lifecycle**: `create_memory_tier_entry` → `convert_to_storage` (marks SSD offset) → `convert_memory_tier_to_block` (flips to BlockDevice). Reverse: `promote_block_to_memory_tier`.
