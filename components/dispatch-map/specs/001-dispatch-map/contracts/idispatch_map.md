# Interface Contract: IDispatchMap

**Crate**: interfaces (`components/interfaces/src/idispatch_map.rs`)
**Feature gate**: `#[cfg(feature = "spdk")]`

> **Synced 2026-07-22**: This contract previously described a `Staging`/`DmaBuffer`-based
> API (`set_dma_alloc`, `create_staging`) that no longer exists in the implementation.
> The current API uses externally-allocated memory-tier pointers
> (`create_memory_tier_entry`) and delegates eviction ordering to the `IEvictionPolicy`
> receptacle rather than internal RDTSC timestamps. See `.specify/sync/drift-report.md`.

## Types

```rust
pub type CacheKey = u64;

pub enum LookupResult {
    NotExist,
    MismatchSize,           // reserved for future use; not currently triggered (see FR-004)
    BlockDevice { offset: u64 },
    MemoryTier { pointer: *mut u8, size: u32 },
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
    RefCountOverflow(CacheKey),
    NoWriteReference(CacheKey),
    InvalidState(String),
}
```

## Interface Methods

| Method | Signature | Semantics |
|--------|-----------|-----------|
| `initialize` | `(&self) -> Result<(), DispatchMapError>` | Requires the `IEvictionPolicy` receptacle to be bound (errors otherwise). If `IExtentManager` is also bound, recovers committed extents via `for_each_extent`; if not bound, succeeds with an empty map. Must be called explicitly — not invoked from the constructor. |
| `lookup` | `(&self, key: CacheKey) -> Result<LookupResult, DispatchMapError>` | Return location (`NotExist`/`BlockDevice`/`MemoryTier`); blocks (hardcoded 2s timeout) until write_ref=0 or timeout. Increments read_ref and refreshes eviction priority on success. Returns `Timeout` on deadline. |
| `create_memory_tier_entry` | `(&self, key: CacheKey, pointer: *mut u8, size: u32) -> Result<(), DispatchMapError>` | Create a `MemoryTier` entry, write_ref=1, registers with `IEvictionPolicy`. Error on `size == 0` (`InvalidSize`) or key exists (`AlreadyExists`). |
| `convert_to_storage` | `(&self, key: CacheKey, offset: u64) -> Result<(), DispatchMapError>` | Sets `ssd_offset` on a `MemoryTier` entry (does not transition state). Conditionally decrements read_ref (only if > 0). Error if key not found or entry is already `BlockDevice`. |
| `convert_memory_tier_to_block` | `(&self, key: CacheKey) -> Result<(), DispatchMapError>` | Transition `MemoryTier { ssd_offset: Some(offset), .. }` → `BlockDevice { offset }`. Error (`InvalidState`) if not `MemoryTier` or `ssd_offset` is `None`. |
| `promote_block_to_memory_tier` | `(&self, key: CacheKey, pointer: *mut u8, size: u32) -> Result<(), DispatchMapError>` | Transition `BlockDevice { offset }` → `MemoryTier { pointer, size, ssd_offset: Some(offset) }` **in place**, preserving the eviction handle and ALL active reference counts. Error on `KeyNotFound`, `InvalidSize` (`size == 0`), or `InvalidState` (already `MemoryTier`). |
| `try_evict_to_block` | `(&self, key: CacheKey) -> Result<(), DispatchMapError>` | Atomically (single lock hold) checks evictability — `MemoryTier` with `ssd_offset: Some(_)` and `read_ref == 0 && write_ref == 0` — and transitions to `BlockDevice { offset }`. Error on `KeyNotFound`, or `InvalidState` if not evictable (active refs, no `ssd_offset`, or not `MemoryTier`). No partial state change on error. |
| `is_evictable` | `(&self, key: CacheKey) -> bool` | Non-erroring predicate form of the check performed by `try_evict_to_block`. Returns `false` (not an error) for non-existent keys. |
| `take_read` | `(&self, key: CacheKey) -> Result<(), DispatchMapError>` | Wait (hardcoded 2s timeout) for write_ref=0, then increment read_ref. Returns `Timeout` on deadline. |
| `take_write` | `(&self, key: CacheKey) -> Result<(), DispatchMapError>` | Wait (hardcoded 2s timeout) for read_ref=0 and write_ref=0, then set write_ref=1. Returns `Timeout` on deadline. |
| `release_read` | `(&self, key: CacheKey) -> Result<(), DispatchMapError>` | Decrement read_ref. Error (`RefCountUnderflow`) if already 0. Notifies blocked writers via condvar. |
| `release_write` | `(&self, key: CacheKey) -> Result<(), DispatchMapError>` | Set write_ref=0. Error (`RefCountUnderflow`) if already 0. Notifies blocked readers/writers via condvar. |
| `downgrade_reference` | `(&self, key: CacheKey) -> Result<(), DispatchMapError>` | Atomically: write_ref=0, read_ref+=1. Error (`NoWriteReference`) if no write ref held. Notifies via condvar. |
| `remove` | `(&self, key: CacheKey) -> Result<(), DispatchMapError>` | Delete entry. Error (`ActiveReferences`) if any refs active; error (`KeyNotFound`) if key not found. |
| `touch` | `(&self, key: CacheKey) -> Result<(), DispatchMapError>` | Refresh the entry's eviction priority via `IEvictionPolicy::touch(handle)` without taking any reference. Error (`KeyNotFound`) if key not found. |
| `oldest_keys` | `(&self, n: usize) -> Vec<CacheKey>` | Delegates to `IEvictionPolicy::get_eviction_candidates(pool, n)`; returns up to `n` keys oldest-first. Used by the dispatcher's eviction logic to select victims. |
| `entry_size` | `(&self, key: CacheKey) -> Result<u32, DispatchMapError>` | Returns the stored size in block-aligned bytes (`size_blocks * 4096`) without acquiring any reference. Error (`KeyNotFound`) if key not found. |
| `recover_extent` | `(&self, key: CacheKey, offset: u64, size_blocks: u32) -> Result<(), DispatchMapError>` | Directly inserts a `BlockDevice` entry and registers it with `IEvictionPolicy`, for incremental recovery. Error (`AlreadyExists`) if key already present. |
| `set_checksum` *(feature `integrity-check`)* | `(&self, key: CacheKey, checksum: u32) -> Result<(), DispatchMapError>` | Records a CRC-32 on the entry so it travels with the index across demote/promote. Error (`KeyNotFound`) if key absent. Present only under the `integrity-check` feature. |
| `get_checksum` *(feature `integrity-check`)* | `(&self, key: CacheKey) -> Option<u32>` | Returns the recorded CRC-32, or `None` if the key is absent or no checksum recorded (a stored `0` is treated as unset). Present only under the `integrity-check` feature. |

## Invariants

1. `write_ref` is always 0 or 1.
2. If `write_ref == 1`, `read_ref == 0` (enforced by `take_write` precondition).
3. `downgrade_reference` transitions atomically: no window where both refs are 0.
4. `remove` is only valid when `read_ref == 0` and `write_ref == 0`.
5. All methods are thread-safe (`&self` with internal synchronization via `Mutex` + `Condvar`).
6. `lookup` refreshes the entry's eviction priority (via the bound `IEvictionPolicy`) on success, making it the "newest" for eviction ordering.
7. `touch` refreshes eviction priority without acquiring any reference — lightweight priority refresh.
8. `promote_block_to_memory_tier` and `try_evict_to_block` never leave an entry in a partially-transitioned state: on error, the entry's location and reference counts are unchanged from before the call.
9. `promote_block_to_memory_tier` preserves the entry's `EvictionHandle` and reference counts across the `BlockDevice → MemoryTier` transition — it does not remove/reinsert the entry, so it is safe to call while the entry is pinned (`read_ref > 0`).
10. `try_evict_to_block` performs its evictability check and its `MemoryTier → BlockDevice` transition under a single lock hold, so no other thread can acquire a reference between the check and the transition.

## Error Semantics

All error conditions return `Err(DispatchMapError)` — no panics, no silent no-ops. The caller is expected to handle errors explicitly.
