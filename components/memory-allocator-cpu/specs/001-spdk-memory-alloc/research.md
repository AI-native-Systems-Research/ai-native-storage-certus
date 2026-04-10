# Research: SPDK CPU Memory Allocator Component

**Date**: 2026-04-10
**Feature**: 001-spdk-memory-alloc

## Research Tasks

### 1. SPDK DMA Allocation Functions Available in Bindings

**Decision**: Use `spdk_dma_zmalloc` / `spdk_dma_free` for non-NUMA-pinned allocations, and `spdk_zmalloc` / `spdk_free` with `SPDK_MALLOC_DMA` flag for NUMA-pinned allocations.

**Rationale**: The `spdk-sys` crate's `build.rs` only exposes four allocation-related functions:
- `spdk_dma_zmalloc(size, align, phys_addr_out)` — allocate zero-init DMA memory from any NUMA node
- `spdk_dma_free(ptr)` — free DMA memory allocated by `spdk_dma_zmalloc`
- `spdk_zmalloc(size, align, phys_addr_out, numa_id, flags)` — allocate zero-init memory on a specific NUMA node
- `spdk_free(ptr)` — free memory allocated by `spdk_zmalloc`

**Alternatives considered**:
- `spdk_dma_malloc` (non-zero-init): Not in bindings. Would need to add to `build.rs` allowlist. Not needed since zmalloc is acceptable for all use cases.
- `spdk_dma_realloc`: Not in SPDK's public API. Realloc must be implemented as alloc + memcpy + free.
- `spdk_malloc` (non-zero, NUMA-pinned): Not in bindings. Same rationale — zmalloc sufficient.

### 2. DmaBuffer Construction Pattern

**Decision**: Use `DmaBuffer::new()` for allocations, which already calls `spdk_dma_zmalloc` / `spdk_zmalloc` internally. The component wraps DmaBuffer construction and adds stats tracking.

**Rationale**: `DmaBuffer::new(size, align, numa_node)` in `interfaces/src/spdk_types.rs` already handles the SPDK function dispatch:
- `numa_node: None` → `spdk_dma_zmalloc` + `spdk_dma_free`
- `numa_node: Some(id)` → `spdk_zmalloc` with `SPDK_MALLOC_DMA` flag + `spdk_free`

This means the component doesn't need to call SPDK functions directly — it delegates to `DmaBuffer::new()` and layers stats on top. This is cleaner and avoids duplicating FFI logic.

**Alternatives considered**:
- Direct FFI calls to spdk_sys: More control but duplicates logic already in DmaBuffer. Rejected for DRY.
- Custom allocator trait: Over-engineered for this use case. Rejected.

### 3. Reallocation Strategy

**Decision**: Implement reallocate as: allocate new buffer → copy min(old_len, new_len) bytes → free old buffer (updating stats for both operations atomically).

**Rationale**: SPDK has no realloc API. The copy approach is standard for DMA memory where in-place growth is not possible (hugepage-backed). The copy uses `std::ptr::copy_nonoverlapping` for safety and performance.

**Alternatives considered**:
- Return error on realloc: Too restrictive; users need resize capability.
- In-place resize: Not possible with SPDK hugepage allocator.

### 4. Interface Location

**Decision**: Define `IMemoryManagement` trait in the `interfaces` crate (`src/imemory_management.rs`) under the `spdk` feature gate. Add `MemoryAllocatorError` to `spdk_types.rs`. Add `AllocationStats` and `ZoneStats` to `spdk_types.rs`.

**Rationale**: Follows the established pattern where `ISPDKEnv` and `IBlockDevice` are defined in the `interfaces` crate, keeping interface definitions centralized and decoupled from implementations.

**Alternatives considered**:
- Define interface in the memory-allocator-cpu crate: Would require other components to depend on the implementation crate. Rejected.

### 5. Thread Safety for Stats

**Decision**: Use `Mutex<StatsInner>` to protect the stats HashMap. Lock is acquired only for stats mutations (after SPDK alloc/free completes), not during the SPDK call itself.

**Rationale**: SPDK allocation functions are thread-safe internally. The component only needs to protect its own bookkeeping (counters). Holding the lock during the SPDK call would unnecessarily serialize allocations.

**Alternatives considered**:
- RwLock: Stats writes happen on every alloc/free, so write-heavy workload makes RwLock's overhead not worthwhile vs Mutex.
- AtomicU64 per counter: Would need one atomic per zone per metric — complex and doesn't naturally handle HashMap growth for new zones.
- Lock-free concurrent HashMap: Over-engineered; Mutex is sufficient given the lock is held for ~nanoseconds of counter updates.

### 6. Stats Data Model

**Decision**: Stats snapshot (`AllocationStats`) contains:
- `total_bytes_allocated: u64` — aggregate across all zones
- `total_allocation_count: u64` — aggregate active allocation count
- `zones: HashMap<i32, ZoneStats>` — per-NUMA-node breakdown

`ZoneStats` contains:
- `bytes_allocated: u64`
- `allocation_count: u64`

Zone key is NUMA node ID (`i32`), with `-1` for "any/unknown" (per clarification).

**Rationale**: Matches the DmaBuffer `numa_node` field type (i32). HashMap allows dynamic zone discovery as allocations happen on new NUMA nodes.

**Alternatives considered**:
- BTreeMap: Ordered but unnecessary for integer keys; HashMap is faster for lookups.
- Fixed-size array indexed by NUMA node: Would require knowing max NUMA nodes at compile time. Rejected.

## All NEEDS CLARIFICATION: Resolved

No unresolved items.
