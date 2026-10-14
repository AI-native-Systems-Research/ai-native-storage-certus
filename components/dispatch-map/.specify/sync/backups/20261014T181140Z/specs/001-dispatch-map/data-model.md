# Data Model: Dispatch Map Component

**Date**: 2026-04-27
**Synced**: 2026-07-22 (spec-sync backfill — see `.specify/sync/drift-report.md`)

> This document previously described a `Staging { buffer: Arc<DmaBuffer> }` location
> variant and an RDTSC `tsc` timestamp field, neither of which exist in the current
> implementation. It has been rewritten to match `components/dispatch-map/src/entry.rs`
> and `state.rs` as of this sync.

## Entities

### CacheKey

Type alias for `u64`. Uniquely identifies an extent in the dispatch map. Matches `ExtentKey` from the extent manager.

### Location

Enum representing where extent data resides.

| Variant | Fields | Description |
|---------|--------|-------------|
| BlockDevice | `offset: u64` | Byte offset on the block device |
| MemoryTier | `pointer: *mut u8`, `size: u32`, `ssd_offset: Option<u64>` | Externally-allocated DRAM pointer, size in bytes, and (once write-through completes) the SSD offset it was — or will be — written to |

Because `Location` is a Rust `enum`, the compiler sizes it to fit its largest variant (`MemoryTier`) regardless of which variant is active at runtime — the struct size does **not** vary per-instance based on the active variant (see `SC-004` in `spec.md`, flagged as spec-wording drift in `.specify/sync/drift-report.md`).

Transitions are bidirectional as of FR-025/FR-026:
- `MemoryTier → BlockDevice` via `convert_memory_tier_to_block` (requires `ssd_offset: Some`) or `try_evict_to_block` (same precondition, plus atomically checks zero refs first).
- `BlockDevice → MemoryTier` via `promote_block_to_memory_tier` (in place — preserves the eviction handle and all reference counts; sets `ssd_offset` to the original block offset).

### DispatchEntry

Per-key metadata stored in the hash map (`components/dispatch-map/src/entry.rs`).

| Field | Type | Description |
|-------|------|-------------|
| location | `Location` | Where the data resides |
| size_blocks | `u32` | Extent size in 4KiB blocks (`entry_size()` returns `size_blocks * 4096`) |
| read_ref | `u32` | Active reader count |
| write_ref | `u32` | Active writer count (0 or 1) |
| eviction_handle | `EvictionHandle` | Opaque handle into the `IEvictionPolicy` component; LRU ordering is delegated externally, not tracked internally |
| reuse_count | `AtomicU32` | Incremented on `lookup`/`take_read`/`downgrade_reference`. **Not currently read or exposed via any `IDispatchMap` method** — see `.specify/sync/align-tasks.md` for the disposition of this dead metric |

There is no internal timestamp (`tsc` or otherwise) on `DispatchEntry`. LRU/eviction ordering is fully delegated to the `IEvictionPolicy` receptacle via `eviction_handle`; `touch()` and `lookup()` refresh priority by calling into that component rather than updating a local field.

### DispatchMapState

Internal synchronization wrapper (not exposed via interface). See `components/dispatch-map/src/state.rs`.

| Field | Type | Description |
|-------|------|-------------|
| inner | `Mutex<Inner>` | Protected struct containing the `entries: HashMap<CacheKey, DispatchEntry>` |
| condvar | `Condvar` | Wakes threads blocked on ref-count conditions (via `wait_for`) |
| pool_id | `Mutex<Option<PoolId>>` | Eviction-policy pool identifier, established on `initialize()` |

The implementation uses a single `Mutex` protecting the entire entries map; there is no separate side-table for buffers (the memory-tier pointer lives directly inside the `Location::MemoryTier` variant).

### LookupResult

Return type for `lookup()`.

| Variant | Fields | Description |
|---------|--------|-------------|
| NotExist | — | Key not found in map |
| MismatchSize | — | Reserved for future use; not currently triggered (`lookup` takes no expected-size parameter — see FR-004) |
| BlockDevice | `offset: u64` | Byte offset on the block device |
| MemoryTier | `pointer: *mut u8`, `size: u32` | Memory-tier pointer and size |

### DispatchMapError

Error enum for all `IDispatchMap` operations (`components/interfaces/src/idispatch_map.rs`).

| Variant | Fields | When |
|---------|--------|------|
| KeyNotFound | key: CacheKey | Operation on non-existent key |
| AlreadyExists | key: CacheKey | `create_memory_tier_entry` / `recover_extent` on a key that already exists |
| ActiveReferences | key: CacheKey | `remove` while refs > 0 |
| Timeout | key: CacheKey | Blocking wait (`lookup`/`take_read`/`take_write`) exceeded the 2s deadline |
| AllocationFailed | msg: String | Reserved; not currently returned by any method (memory-tier allocation is external to the dispatch map) |
| InvalidSize | — | `create_memory_tier_entry` / `promote_block_to_memory_tier` with `size == 0` |
| NotInitialized | msg: String | Reserved; not currently returned (`initialize()` errors via a missing-`IEvictionPolicy` failure instead) |
| RefCountUnderflow | key: CacheKey | `release_read`/`release_write` when the corresponding count is already 0 |
| RefCountOverflow | key: CacheKey | Reference acquisition when the corresponding count is already at `u32::MAX` |
| NoWriteReference | key: CacheKey | `downgrade_reference` without a write ref held |
| InvalidState | msg: String | `convert_to_storage`/`convert_memory_tier_to_block` on a non-matching entry state, or `promote_block_to_memory_tier`/`try_evict_to_block` preconditions not met |

Note: there is no dedicated null-pointer variant. `create_memory_tier_entry` does not currently validate its `pointer` argument — see the null-pointer gap tracked in `.specify/sync/align-tasks.md`.

## State Machine

```
       recover_extent(key, offset, size_blocks)        create_memory_tier_entry(key, ptr, size)
                     │                                              │
                     ▼                                              ▼
      ┌───────────────────────────┐   promote_block_to_memory_tier  ┌───────────────────────────┐
      │        BlockDevice        │ ───────────────────────────────>│         MemoryTier         │
      │  offset                   │<─────────────────────────────── │  pointer, size, ssd_offset │
      └──────────────┬────────────┘   convert_memory_tier_to_block  └──────────────┬────────────┘
                     │                 or try_evict_to_block (atomic,               │
                     │                 requires ssd_offset: Some + zero refs)       │
                     │                                                              │
                     └───────────────────── remove(key) [requires all refs=0] ──────┘
                                                       │
                                                       ▼
                                              (entry deleted)
```

`convert_to_storage(key, offset)` does **not** itself transition `MemoryTier → BlockDevice`; it only sets the entry's `ssd_offset` field (enabling the subsequent `convert_memory_tier_to_block` / `try_evict_to_block` transition) and conditionally decrements `read_ref`.

Recovery path: `initialize()` → entries created directly as `BlockDevice` (from `IExtentManager::for_each_extent` iteration). `recover_extent()` provides the same `BlockDevice` insertion for incremental, single-extent recovery.

## Relationships

```
DispatchMapComponent
    ├── provides: IDispatchMap
    ├── receptacle: ILogger (diagnostics)
    ├── receptacle: IExtentManager (optional; recovery via for_each_extent)
    ├── receptacle: IEvictionPolicy (mandatory; LRU ordering for touch()/oldest_keys()/eviction_handle)
    └── internal: DispatchMapState
                    ├── inner: Mutex<{ entries: HashMap<CacheKey, DispatchEntry> }>
                    ├── condvar: Condvar
                    └── pool_id: Mutex<Option<PoolId>>
```
