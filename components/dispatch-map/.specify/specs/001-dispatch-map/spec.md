# Feature Specification: Dispatch Map

**Feature Branch**: `001-dispatch-map`
**Created**: 2026-07-08
**Status**: Backfilled
**Source**: Generated from existing implementation

## Backfill Notice

> This spec was generated from existing code via `speckit.sync.backfill`.
> It documents current behavior, not original intent.
> Review carefully and update to reflect desired behavior.

## Overview

The Dispatch Map component (`dispatch-map`) is a thread-safe, in-memory routing table for the Certus storage system. It maps extent keys (`CacheKey`, a `u64`) to their current physical location -- either a pointer in the DRAM memory-tier pool or a byte offset on a block device -- with per-entry readers-writer reference counting that enables safe concurrent access without data races.

The component implements the `IDispatchMap` interface using the Certus component framework (`define_component!`) and declares receptacles for `ILogger` (diagnostic output), `IExtentManager` (crash recovery of committed extents), and `IEvictionPolicy` (LRU eviction ordering). It provides timeout-based blocking (2 second default) on contention, atomic write-to-read reference downgrade, state lifecycle transitions (memory-tier to block-device via write-through), and LRU-driven eviction support through integration with the eviction policy component.

## User Scenarios & Testing

### User Story 1 - Extent Lookup (Priority: P1)

As a **dispatcher** handling a read request, I want to look up an extent's current location by key, so that I can route the I/O to the correct data path (memory-tier or block device).

**Acceptance Scenarios**:

- **Given** a key exists in the map with no active writer, **When** `lookup(key)` is called, **Then** a `LookupResult::MemoryTier` or `LookupResult::BlockDevice` is returned and the read reference is incremented by 1.
- **Given** a key does not exist, **When** `lookup(key)` is called, **Then** `LookupResult::NotExist` is returned immediately without blocking.
- **Given** a key has an active writer (write_ref > 0), **When** `lookup(key)` is called, **Then** the call blocks up to 2 seconds, and returns `Timeout` if the writer does not release.
- **Given** a successful lookup, **When** the caller is done with the data, **Then** the caller must call `release_read(key)` to decrement the read reference.

### User Story 2 - Memory-Tier Entry Creation (Priority: P1)

As a **memory-tier manager**, I want to register a new extent in the dispatch map with its DRAM pointer, so that subsequent lookups can locate the data in memory.

**Acceptance Scenarios**:

- **Given** the key does not exist, **When** `create_memory_tier_entry(key, pointer, size)` is called with size > 0, **Then** the entry is created with `write_ref=1` (caller holds exclusive access) and tracked in the eviction policy.
- **Given** the key already exists, **When** `create_memory_tier_entry` is called, **Then** `AlreadyExists` error is returned.
- **Given** size is 0, **When** `create_memory_tier_entry` is called, **Then** `InvalidSize` error is returned.

### User Story 3 - Write-Through and Eviction Lifecycle (Priority: P1)

As a **flush/eviction manager**, I want to transition a memory-tier entry through write-through to block-device state, so that the DRAM buffer can be reclaimed while the data remains accessible on SSD.

**Acceptance Scenarios**:

- **Given** a memory-tier entry with no `ssd_offset`, **When** `convert_to_storage(key, offset)` is called, **Then** the `ssd_offset` field is set (but the entry remains in MemoryTier state) and the read reference is decremented by 1.
- **Given** a memory-tier entry with `ssd_offset` set, **When** `convert_memory_tier_to_block(key)` is called, **Then** the entry transitions to `BlockDevice` state with the previously recorded offset.
- **Given** a memory-tier entry without `ssd_offset`, **When** `convert_memory_tier_to_block(key)` is called, **Then** `InvalidState` error is returned.
- **Given** an entry already in `BlockDevice` state, **When** `convert_to_storage(key, offset)` is called, **Then** `InvalidState` error is returned.

### User Story 4 - Reference Counting (Priority: P1)

As a **concurrent accessor**, I want to take and release read/write references on entries, so that I can safely coordinate exclusive writes and shared reads without data corruption.

**Acceptance Scenarios**:

- **Given** an entry with write_ref=0, **When** `take_read(key)` is called, **Then** read_ref is incremented by 1.
- **Given** an entry with write_ref > 0, **When** `take_read(key)` is called, **Then** the call blocks up to 2s and returns `Timeout` if the writer does not release.
- **Given** an entry with read_ref=0 and write_ref=0, **When** `take_write(key)` is called, **Then** write_ref is set to 1.
- **Given** an entry with any active references, **When** `take_write(key)` is called, **Then** the call blocks up to 2s and returns `Timeout` if references are not released.
- **Given** an entry with read_ref=0, **When** `release_read(key)` is called, **Then** `RefCountUnderflow` error is returned.
- **Given** an entry with write_ref=0, **When** `release_write(key)` is called, **Then** `RefCountUnderflow` error is returned.

### User Story 5 - Reference Downgrade (Priority: P2)

As a **writer completing a staged write**, I want to atomically downgrade my write reference to a read reference, so that concurrent readers can begin accessing the data while I retain a read reference.

**Acceptance Scenarios**:

- **Given** an entry with write_ref=1, **When** `downgrade_reference(key)` is called, **Then** write_ref becomes 0 and read_ref is incremented by 1.
- **Given** an entry with write_ref=0, **When** `downgrade_reference(key)` is called, **Then** `NoWriteReference` error is returned.
- **Given** pending readers blocked by the writer, **When** downgrade completes, **Then** blocked readers are unblocked via condvar notification.

### User Story 6 - Entry Removal (Priority: P1)

As an **eviction manager**, I want to remove entries from the dispatch map, so that reclaimed extents are no longer routable.

**Acceptance Scenarios**:

- **Given** an entry with read_ref=0 and write_ref=0, **When** `remove(key)` is called, **Then** the entry is deleted and removed from the eviction policy.
- **Given** an entry with active references, **When** `remove(key)` is called, **Then** `ActiveReferences` error is returned.
- **Given** a key that does not exist, **When** `remove(key)` is called, **Then** `KeyNotFound` error is returned.

### User Story 7 - Eviction Ordering (Priority: P2)

As an **eviction policy consumer**, I want to retrieve the N oldest (least-recently-used) keys, so that I can select entries for eviction based on access recency.

**Acceptance Scenarios**:

- **Given** entries created in order [1, 2, 3], **When** `oldest_keys(2)` is called, **Then** keys [1, 2] are returned (oldest first).
- **Given** entry 1 has been touched/looked up more recently than entry 2, **When** `oldest_keys(2)` is called, **Then** entry 1 is NOT in the result set.
- **Given** fewer than N entries exist, **When** `oldest_keys(N)` is called, **Then** all entries are returned.
- **Given** an empty map, **When** `oldest_keys(N)` is called, **Then** an empty vector is returned.

### User Story 8 - Crash Recovery (Priority: P1)

As the **system on restart**, I want to rebuild the dispatch map from persisted extents in the extent manager, so that previously committed data is immediately accessible.

**Acceptance Scenarios**:

- **Given** an extent manager with 3 committed extents, **When** `initialize()` is called, **Then** 3 entries are created as `BlockDevice` locations with zero read/write references and tracked in the eviction policy.
- **Given** no extent manager is bound, **When** `initialize()` is called, **Then** the map starts empty without error.
- **Given** an extent manager with zero extents, **When** `initialize()` is called, **Then** the map starts empty without error.

### User Story 9 - Evictability Check (Priority: P2)

As an **eviction manager**, I want to check whether a specific entry is safe to evict, so that I only evict entries that have been written through and are not actively referenced.

**Acceptance Scenarios**:

- **Given** a memory-tier entry with `ssd_offset: Some(_)` and zero references, **When** `is_evictable(key)` is called, **Then** `true` is returned.
- **Given** a memory-tier entry with active references, **When** `is_evictable(key)` is called, **Then** `false` is returned.
- **Given** a memory-tier entry without `ssd_offset`, **When** `is_evictable(key)` is called, **Then** `false` is returned.
- **Given** a key that does not exist, **When** `is_evictable(key)` is called, **Then** `false` is returned.

### User Story 10 - Extent Recovery Insertion (Priority: P2)

As the **recovery subsystem**, I want to insert recovered extents directly as block-device entries, so that I can rebuild the map without DMA buffer allocation.

**Acceptance Scenarios**:

- **Given** the key does not exist, **When** `recover_extent(key, offset, size_blocks)` is called, **Then** the entry is created as `BlockDevice` with the given offset and zero references.
- **Given** the key already exists, **When** `recover_extent` is called, **Then** `AlreadyExists` error is returned.

## Requirements

### Functional Requirements

- **FR-001**: The component MUST provide the `IDispatchMap` interface via the component framework's `define_component!` macro.
- **FR-002**: Each entry MUST track its location as either `MemoryTier { pointer, size, ssd_offset }` or `BlockDevice { offset }`.
- **FR-003**: Each entry MUST maintain separate `read_ref` (u32) and `write_ref` (u32) counters. Write reference MUST be binary (0 or 1).
- **FR-004**: `lookup(key)` MUST block (up to timeout) if `write_ref > 0`, then increment `read_ref` and return the location.
- **FR-005**: `take_read(key)` MUST block until `write_ref == 0`, then increment `read_ref`.
- **FR-006**: `take_write(key)` MUST block until both `read_ref == 0` and `write_ref == 0`, then set `write_ref = 1`.
- **FR-007**: `release_read(key)` MUST decrement `read_ref` and notify waiters. MUST fail with `RefCountUnderflow` if `read_ref == 0`.
- **FR-008**: `release_write(key)` MUST set `write_ref = 0` and notify waiters. MUST fail with `RefCountUnderflow` if `write_ref == 0`.
- **FR-009**: `downgrade_reference(key)` MUST atomically set `write_ref = 0` and increment `read_ref`, then notify waiters. MUST fail with `NoWriteReference` if `write_ref == 0`.
- **FR-010**: `remove(key)` MUST fail with `ActiveReferences` if any references are held. On success, MUST remove from eviction policy.
- **FR-011**: `create_memory_tier_entry(key, ptr, size)` MUST reject duplicate keys (`AlreadyExists`) and zero size (`InvalidSize`). MUST set `write_ref = 1` on success and track in eviction policy.
- **FR-012**: `convert_to_storage(key, offset)` MUST set `ssd_offset` on a MemoryTier entry, decrement `read_ref`, and notify waiters. MUST fail with `InvalidState` if entry is already BlockDevice.
- **FR-013**: `convert_memory_tier_to_block(key)` MUST transition MemoryTier entries to BlockDevice using the stored `ssd_offset`. MUST fail with `InvalidState` if `ssd_offset` is `None` or entry is not MemoryTier.
- **FR-014**: `initialize()` MUST iterate all extents from bound `IExtentManager` and insert them as `BlockDevice` entries with zero references. MUST succeed with empty map if no extent manager is bound.
- **FR-015**: `oldest_keys(n)` MUST return up to `n` keys in least-recently-used order via the eviction policy.
- **FR-016**: `touch(key)` MUST update the eviction policy recency for the entry without taking any reference.
- **FR-017**: `entry_size(key)` MUST return `size_blocks * 4096` for the entry.
- **FR-018**: `is_evictable(key)` MUST return `true` only when the entry is MemoryTier with `ssd_offset: Some(_)` AND has zero read/write references.
- **FR-019**: `recover_extent(key, offset, size_blocks)` MUST insert a BlockDevice entry with zero references and track in eviction policy. MUST reject duplicate keys.

### Non-Functional Requirements

- **NFR-001**: All blocking operations MUST timeout after 2 seconds (configurable via `DEFAULT_TIMEOUT` constant). Timeout MUST NOT corrupt internal state.
- **NFR-002**: The data structure MUST be safe for concurrent access from multiple threads. Internal state MUST be protected by `Mutex` with `Condvar` for blocking.
- **NFR-003**: `DispatchEntry` size MUST NOT exceed 56 bytes (validated by benchmark assertion) to maintain cache-line efficiency.
- **NFR-004**: Lookup without contention MUST complete in sub-microsecond time (benchmarked via Criterion).
- **NFR-005**: Reference take/release operations MUST complete in sub-microsecond time without contention.
- **NFR-006**: The component MUST NOT require SPDK hardware at runtime; persistence is delegated to the `IExtentManager` receptacle.
- **NFR-007**: All internal pointer handling (`Location::MemoryTier`) MUST have explicit `// SAFETY:` justification and implement `Send + Sync` correctly.
- **NFR-008**: The component MUST handle graceful degradation when optional receptacles (`ILogger`, `IExtentManager`) are not bound.
- **NFR-009**: The eviction policy pool MUST be lazily created on first use and cached for the component's lifetime.

## Key Entities

| Entity | Type | Description |
|--------|------|-------------|
| `CacheKey` | `u64` | Unique identifier for an extent in the map |
| `DispatchEntry` | struct | Per-key metadata: location, size_blocks, read_ref, write_ref, eviction_handle |
| `Location` | enum | `BlockDevice { offset }` or `MemoryTier { pointer, size, ssd_offset }` |
| `LookupResult` | enum | `NotExist`, `BlockDevice { offset }`, `MemoryTier { pointer, size }` |
| `DispatchMapError` | enum | Typed errors: KeyNotFound, AlreadyExists, ActiveReferences, Timeout, etc. |
| `DispatchMapState` | struct | Thread-safe state: `Mutex<Inner>`, `Condvar`, `Mutex<Option<PoolId>>` |
| `Inner` | struct | Protected map: `HashMap<CacheKey, DispatchEntry>` |
| `EvictionHandle` | opaque | Handle into the eviction policy's LRU ordering per entry |
| `PoolId` | opaque | Identifier for this component's eviction pool |

## Dependencies

| Dependency | Type | Purpose |
|-----------|------|---------|
| `component-framework` | Build | Provides `define_component!` macro and component wiring |
| `component-core` | Build | Core traits (`IUnknown`, `query_interface!`) |
| `component-macros` | Build | Procedural macros for interface/component definitions |
| `interfaces` (feature: `spdk`) | Build | Defines `IDispatchMap`, `ILogger`, `IExtentManager`, `IEvictionPolicy` traits |
| `ILogger` | Receptacle (optional) | Diagnostic logging; component degrades gracefully if unbound |
| `IExtentManager` | Receptacle (optional) | Provides persisted extents for recovery; map starts empty if unbound |
| `IEvictionPolicy` | Receptacle (required) | Provides LRU eviction pool; required for entry creation and ordering |
| `eviction-policy-lru` | Dev dependency | Concrete eviction policy used in tests and benchmarks |
| `criterion` | Dev dependency | Benchmarking framework |

## Success Criteria

1. All 10 formally verified properties (P1-P10) pass via Creusot verification.
2. All unit tests pass (`cargo test -p dispatch-map`), covering:
   - Reference counting: increment, decrement, overflow, underflow
   - Blocking semantics: writer blocks readers, readers block writer, timeout behavior
   - State transitions: MemoryTier -> (ssd_offset set) -> BlockDevice
   - Recovery: populated and empty extent managers
   - Eviction ordering: creation order, LRU update on touch/lookup
3. All integration tests pass, covering:
   - Multi-threaded concurrent access (4 readers, writer blocking, independent keys)
   - Downgrade unblocks pending readers
   - Sequential writers succeed
   - Remove blocked by active references
4. Benchmark baselines maintained:
   - `lookup_no_contention`: sub-microsecond
   - `take_release_read` / `take_release_write`: sub-microsecond
   - `oldest_keys` with 1000 entries: reasonable latency
   - `DispatchEntry` size <= 56 bytes
5. `cargo clippy -p dispatch-map -- -D warnings` passes with no warnings.
6. `cargo doc -p dispatch-map --no-deps` produces warning-free documentation.

## Implementation Notes

- **Concurrency model**: Single `Mutex<Inner>` + `Condvar` protects all state. Blocking operations (`lookup`, `take_read`, `take_write`) use `wait_for()` which loops on the condvar with a deadline. All mutating operations call `condvar.notify_all()` after releasing the lock to wake waiters.
- **Eviction integration**: The component lazily creates a pool in the eviction policy on first use (`get_pool_id()`). Every entry creation registers a handle via `ep.track()`, every lookup/touch calls `ep.touch()`, and removal calls `ep.remove()`. The `oldest_keys(n)` method delegates directly to `ep.peek_oldest()`.
- **Memory-tier lifecycle**: Entries begin in `MemoryTier` state with `ssd_offset: None`. After write-through completes, `convert_to_storage` sets `ssd_offset`. A subsequent `convert_memory_tier_to_block` finalizes the transition to `BlockDevice` state. Only entries with `ssd_offset: Some(_)` and zero references are evictable.
- **Recovery path**: `initialize()` iterates the extent manager's persisted extents and inserts each as a `BlockDevice` entry with zero references. This restores the map to a consistent view of committed storage after a crash.
- **Unsafe code**: `Location` implements `Send + Sync` manually because it contains a raw `*mut u8` pointer. Safety is justified by the fact that all access is serialized through the dispatch map's mutex and the pointed-to memory is in a thread-safe memory pool.
- **Component version**: Currently `0.2.0` (as declared in `define_component!`), package version `0.1.0` (Cargo.toml).
