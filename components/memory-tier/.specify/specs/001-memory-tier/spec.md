# Feature Specification: Memory Tier DRAM Cache Pool

**Feature Branch**: `001-memory-tier`
**Created**: 2026-07-08
**Status**: Backfilled
**Source**: Generated from existing implementation

## Backfill Notice

> This spec was generated from existing code via `speckit.sync.backfill`.
> It documents current behavior, not original intent.
> Review carefully and update to reflect desired behavior.

## Overview

The `memory-tier` component provides a DRAM-resident cache pool for the Certus storage system. It allocates a single contiguous memory region (via SPDK hugepages when available, falling back to `mmap`) and manages it with a first-fit free-list allocator. Objects are inserted by a 64-bit `CacheKey` and tracked with LRU ordering via an external `IEvictionPolicy` receptacle, enabling the least-recently-used entries to be evicted when space is needed.

The pool is internally sharded into 16 independent partitions (key modulo 16) to reduce lock contention under concurrent access from multiple dispatcher threads. All allocations are 4 KiB-aligned, making returned pointers suitable for direct NVMe DMA transfers. The component sits between the block-device layer and higher-level dispatch/extent logic, serving as a fast staging area for data that will be read from or written to persistent storage.

## User Scenarios & Testing

### User Story 1 - Insert and Retrieve Cached Data (Priority: P1)

As a dispatcher thread, I want to insert a data block into the memory tier by key and later retrieve a direct pointer to it, so that I can serve read requests from DRAM without hitting the block device.

**Acceptance Scenarios**:
- Given the pool is initialized with 256 KiB, when I insert key=1 with size=4096, then I receive a non-null pointer and `contains(1)` returns true.
- Given key=1 is inserted, when I call `get(1)`, then I receive the same pointer and size.
- Given key=1 is inserted, when I write data through the pointer and read it back via `get(1)`, then the data is intact.

### User Story 2 - Pool Full Triggers Eviction (Priority: P1)

As a dispatcher thread, I want the pool to report when it is full so that I can evict LRU entries and retry the insertion.

**Acceptance Scenarios**:
- Given a shard is fully allocated, when I attempt to insert a new key mapping to that shard, then `PoolFull` is returned.
- Given the pool is full, when I call `evict_lru()`, then the oldest entry is removed and space is freed for a new allocation.

### User Story 3 - LRU Ordering Maintained via Touch (Priority: P1)

As the cache system, I want accessed entries to be promoted in LRU order so that frequently-used entries are retained longer.

**Acceptance Scenarios**:
- Given keys A, B, C are inserted in order, when I `touch(A)` and then call `evict_lru()`, then B is evicted (not A).
- Given keys are inserted, when I call `peek(key)`, then the LRU position is NOT updated (entry remains eviction-eligible in its original position).

### User Story 4 - NUMA-Aware Pool Allocation (Priority: P2)

As a system operator, I want the memory pool bound to a specific NUMA node so that data-path latency is minimized on multi-socket systems.

**Acceptance Scenarios**:
- Given `initialize(pool_size, Some(0))` is called, when the pool is allocated, then `mbind` is invoked to bind pages to NUMA node 0.
- Given `mbind` fails (e.g., invalid node), when initialization proceeds, then the pool is still usable with default memory policy and a warning is logged.

### User Story 5 - DMA-Capable Pool via SPDK (Priority: P2)

As the block-device layer, I want the memory-tier pool to be backed by SPDK hugepages when SPDK is active, so that returned pointers can be used directly for NVMe DMA without intermediate copies.

**Acceptance Scenarios**:
- Given SPDK env is active, when `initialize()` is called, then `spdk_zmalloc` is used and `is_dma_capable()` returns true.
- Given SPDK env is not active, when `initialize()` is called, then `mmap` fallback is used and `is_dma_capable()` returns false.

### User Story 6 - Targeted Shard Eviction (Priority: P2)

As the dispatcher, I want to evict the LRU entry from a specific shard (the one a new key would hash to) so that freed space is guaranteed usable for that key's insertion.

**Acceptance Scenarios**:
- Given key=K maps to shard S, when I call `evict_lru_for_key(K)`, then the evicted entry was in shard S and the freed space is available for `insert(K, ...)`.

### User Story 7 - Batch Touch for Hot-Path Efficiency (Priority: P2)

As the dispatcher handling multiple concurrent reads, I want to update LRU positions for a batch of keys in one call to amortize locking overhead.

**Acceptance Scenarios**:
- Given keys [A, B, C] are inserted, when I call `batch_touch(&[A, B, C])`, then all three are promoted to the most-recently-used position.

### User Story 8 - Clear All Entries (Priority: P3)

As an operator performing a cache reset, I want to clear all entries from the pool so that the full capacity is available again.

**Acceptance Scenarios**:
- Given 5 entries are inserted, when I call `clear()`, then it returns `Ok(5)`, `used()` returns 0, and all keys are no longer present.

## Requirements

### Functional Requirements

- **FR-001**: The component shall allocate a contiguous memory pool of a caller-specified size on `initialize()`. Double-initialization shall return an error.
- **FR-002**: The pool shall prefer hugepage backing (`MAP_HUGETLB`) for TLB efficiency, falling back to regular anonymous pages if hugepages are unavailable.
- **FR-003**: When SPDK env is active (feature `spdk`), the pool shall be allocated via `spdk_zmalloc` with DMA flag, producing DMA-capable pointers.
- **FR-004**: All allocations shall be 4 KiB-aligned (suitable for NVMe sector I/O).
- **FR-005**: `insert(key, size)` shall allocate `size` bytes (rounded up to 4 KiB) and return a pointer into the pool. It shall reject zero size, duplicate keys, and uninitialized state.
- **FR-006**: `get(key)` shall return the pointer and size for an existing key and promote it in LRU order. Returns `None` for missing keys.
- **FR-007**: `peek(key)` shall return the pointer and size without updating LRU position.
- **FR-008**: `evict_lru()` shall remove the least-recently-used entry from the pool, cycling through shards round-robin starting from an atomic counter.
- **FR-009**: `evict_lru_for_key(key)` shall evict the LRU entry from the specific shard that `key` maps to.
- **FR-010**: `remove(key)` shall explicitly remove an entry and free its allocation. Returns `KeyNotFound` for absent keys.
- **FR-011**: `touch(key)` shall promote a key in LRU ordering without returning data.
- **FR-012**: `batch_touch(keys)` shall promote multiple keys in a single operation, amortizing lock acquisition.
- **FR-013**: `contains(key)` shall report whether a slot exists.
- **FR-014**: `capacity()` shall return the total pool size in bytes; `used()` shall return the currently allocated byte count.
- **FR-015**: `pool_info()` shall return the base pointer and size of the pool (for CUDA host registration or similar).
- **FR-016**: `is_dma_capable()` shall return true only when the pool is backed by SPDK hugepages.
- **FR-017**: `clear()` shall remove all entries, reset all shard allocators, clear all eviction-policy pools, and return the count of cleared entries.
- **FR-018**: `oldest_keys(n)` shall return up to `n` keys in oldest-first order without removing them, sampling across shards.
- **FR-019**: When `numa_node` is specified, the pool shall be bound to that NUMA node via `mbind(MPOL_BIND)`. If binding fails, the pool remains usable with default policy and a warning is logged.

### Non-Functional Requirements

- **NFR-001**: The pool shall be internally sharded into 16 partitions (key modulo 16) to minimize lock contention under concurrent access.
- **NFR-002**: Per-shard locking shall use `Mutex` with fine granularity -- no global lock on the data path after initialization.
- **NFR-003**: The allocator shall coalesce adjacent free regions on deallocation to prevent fragmentation.
- **NFR-004**: After initialization, the component shall make no system calls on the allocation data path (all allocations are sub-allocations from the pre-mapped pool).
- **NFR-005**: The data structure for free regions shall use `BTreeMap<offset, size>` for O(log n) first-fit allocation.
- **NFR-006**: The component shall be `Send + Sync` to support multi-threaded dispatcher access.
- **NFR-007**: On drop, the component shall properly unmap/free the pool (via `munmap` or `spdk_free` depending on allocation source).
- **NFR-008**: If SPDK has already shut down at drop time, the pool memory is intentionally leaked rather than calling freed SPDK functions.

## Key Entities

| Entity | Type | Description |
|--------|------|-------------|
| `CacheKey` | `u64` | Unique identifier for a cached object (defined in `interfaces::idispatch_map`) |
| `EvictionHandle` | struct | Opaque handle linking a slot to its position in the eviction policy |
| `PoolId` | `u32` | Identifies an eviction-policy pool (one per shard) |
| `Slot` | struct | Internal record: offset into shard, size, eviction handle |
| `Shard` | struct | Contains a `FreeList` allocator and `HashMap<CacheKey, Slot>` |
| `MemoryTierState` | struct | Holds pool pointer, shard vector, initialization flag, SPDK flag |
| `FreeList` | struct | BTreeMap-based first-fit allocator with coalescing |
| `MemoryTierError` | enum | Error variants: PoolFull, KeyNotFound, AlreadyExists, AllocationFailed, InvalidSize, NotEvictable, NotInitialized |

## Dependencies

| Dependency | Interface | Role |
|------------|-----------|------|
| `IEvictionPolicy` | Receptacle (required) | Provides LRU tracking, promotion, and eviction-candidate selection |
| `ILogger` | Receptacle (optional) | Receives info/warn messages during initialization |
| `libc` | Crate | `mmap`, `munmap`, `mbind` system calls |
| `spdk-sys` | Crate (optional, feature-gated) | `spdk_zmalloc`, `spdk_free` for DMA-capable allocation |
| `component-framework` | Crate | `define_component!` macro, receptacle wiring |
| `interfaces` | Crate (with `spdk` feature) | `IMemoryTier`, `IEvictionPolicy`, `ILogger`, type definitions |

## Success Criteria

1. All unit tests pass (`cargo test -p memory-tier`): initialization, insert/get/remove, duplicate rejection, pool-full behavior, LRU eviction ordering, touch promotion, capacity tracking, clear, peek non-promotion.
2. 10 formally verified properties (P1-P10) discharged via Creusot SMT solvers (21 VCs).
3. No unsafe memory access beyond the documented `mmap`/SPDK pool pointer arithmetic (covered by SAFETY comments).
4. Concurrent access from 16+ threads does not deadlock or corrupt state (per-shard Mutex guarantees).
5. Memory is properly reclaimed on component drop (no leaks except the documented SPDK-shutdown edge case).
6. `clippy` and `cargo doc --no-deps` produce no warnings.

## Implementation Notes

- The shard count (16) is a compile-time constant `NUM_SHARDS`. Changing it requires recompilation.
- Key-to-shard mapping uses simple modulo (`key as usize % 16`), which assumes uniform key distribution. Clustered keys may cause hot shards.
- The eviction round-robin counter (`evict_counter`) uses `AtomicUsize` with `Relaxed` ordering -- exact fairness is not guaranteed under high contention, but approximate round-robin is sufficient.
- The `RwLock<MemoryTierState>` is acquired as a read lock on all data-path operations (insert/get/remove/evict) since the fields it protects are immutable after initialization. The write lock is only taken during `initialize()`.
- Pool size is divided equally among 16 shards. If `pool_size` is not evenly divisible by 16, the remainder bytes are lost (not allocated to any shard).
- The allocator rounds all allocation sizes up to 4 KiB, which means small objects waste up to 4095 bytes of internal fragmentation per slot.
- Component version is `0.2.0` as declared in `define_component!`.
