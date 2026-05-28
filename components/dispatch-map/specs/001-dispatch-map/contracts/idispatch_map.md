# Interface Contract: IDispatchMap

**Crate**: interfaces (`components/interfaces/src/idispatch_map.rs`)
**Feature gate**: `#[cfg(feature = "spdk")]`

## Types

```rust
pub type CacheKey = u64;

pub enum LookupResult {
    NotExist,
    MismatchSize,
    Staging { buffer: Arc<DmaBuffer> },
    BlockDevice { offset: u64 },
}

pub enum DispatchMapError {
    KeyNotFound(CacheKey),
    AlreadyExists(CacheKey),
    ActiveReferences(CacheKey),
    Timeout(CacheKey),
    AllocationFailed(String),
    InvalidSize,
    NotInitialized(String),
    RefCountUnderflow(CacheKey),
    NoWriteReference(CacheKey),
    InvalidState(String),
}
```

## Interface Methods

| Method | Signature | Semantics |
|--------|-----------|-----------|
| `set_dma_alloc` | `(&self, alloc: DmaAllocFn)` | Set the DMA buffer allocator. Must be called before `create_staging`. |
| `initialize` | `(&self) -> Result<(), DispatchMapError>` | Recover committed extents from IExtentManager. Must be called after receptacles are bound. |
| `create_staging` | `(&self, key: CacheKey, size: u32) -> Result<Arc<DmaBuffer>, DispatchMapError>` | Allocate staging buffer, record entry with write_ref=1. Error on size=0, alloc failure, or key exists. |
| `lookup` | `(&self, key: CacheKey) -> Result<LookupResult, DispatchMapError>` | Return location; blocks (internal 2s timeout) until write_ref=0 or timeout. Increments read_ref and updates TSC on success. Returns `Timeout` on deadline. |
| `convert_to_storage` | `(&self, key: CacheKey, offset: u64) -> Result<(), DispatchMapError>` | Transition Staging → BlockDevice. Decrements read_ref. Error if not staging or key not found. |
| `take_read` | `(&self, key: CacheKey) -> Result<(), DispatchMapError>` | Wait (internal 2s timeout) for write_ref=0, then increment read_ref. Returns `Timeout` on deadline. |
| `take_write` | `(&self, key: CacheKey) -> Result<(), DispatchMapError>` | Wait (internal 2s timeout) for read_ref=0 and write_ref=0, then set write_ref=1. Returns `Timeout` on deadline. |
| `release_read` | `(&self, key: CacheKey) -> Result<(), DispatchMapError>` | Decrement read_ref. Error if already 0. Notifies blocked writers via condvar. |
| `release_write` | `(&self, key: CacheKey) -> Result<(), DispatchMapError>` | Set write_ref=0. Error if already 0. Notifies blocked readers/writers via condvar. |
| `downgrade_reference` | `(&self, key: CacheKey) -> Result<(), DispatchMapError>` | Atomically: write_ref=0, read_ref+=1. Error if no write ref held. Notifies via condvar. |
| `remove` | `(&self, key: CacheKey) -> Result<(), DispatchMapError>` | Delete entry. Error if any refs active or key not found. |
| `touch` | `(&self, key: CacheKey) -> Result<(), DispatchMapError>` | Update the entry's TSC timestamp without taking any reference. Error if key not found. Used to refresh eviction priority without performing I/O. |
| `oldest_keys` | `(&self, n: usize) -> Vec<CacheKey>` | Return up to `n` keys sorted by ascending TSC (oldest first). Used by the dispatcher's eviction logic to select victims. |

## Invariants

1. `write_ref` is always 0 or 1.
2. If `write_ref == 1`, `read_ref == 0` (enforced by `take_write` precondition).
3. `downgrade_reference` transitions atomically: no window where both refs are 0.
4. After `convert_to_storage`, the staging buffer is released and subsequent lookups return `BlockDevice`.
5. `remove` is only valid when `read_ref == 0` and `write_ref == 0`.
6. All methods are thread-safe (`&self` with internal synchronization via Mutex + Condvar).
7. `lookup` updates the entry's TSC timestamp on success, making it the "newest" for eviction ordering.
8. `touch` updates TSC without acquiring any reference — lightweight timestamp refresh.

## Error Semantics

All error conditions return `Err(DispatchMapError)` — no panics, no silent no-ops. The caller is expected to handle errors explicitly.
