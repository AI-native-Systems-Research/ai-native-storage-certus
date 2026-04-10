# Feature Specification: SPDK CPU Memory Allocator Component

**Feature Branch**: `001-spdk-memory-alloc`  
**Created**: 2026-04-10  
**Status**: Draft  
**Input**: User description: "Build a component for SPDK-based CPU-based memory allocation. The component should bind to the ISPDKEnv interface provided by an instantiation of spdk-env component. A receptacle should be included. This component uses the DmaBuffer type from components/interfaces crate as the basis for memory handles. The component exposes an interface IMemoryManagement that provides APIs for allocating, reallocating, zmalloc and freeing memory. The component should include stats detailing how much memory, in what zones, has been allocated and how much memory remains. The implementation should use SPDK functions, spdk_dma_xx and APIs should use optional NUMA affinity parameters. Include unit tests and Criterion performance benchmarks."

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Allocate DMA Memory (Priority: P1)

A developer building a storage component needs to allocate DMA-safe memory buffers for I/O operations. They create a MemoryAllocatorCpu component, connect it to the SPDK environment via the ISPDKEnv receptacle, and allocate memory buffers of specified sizes. The returned handles are DmaBuffer instances that can be passed directly to SPDK block device operations.

**Why this priority**: Memory allocation is the fundamental operation that all other functionality depends on. Without allocation, no other memory management operations are useful.

**Independent Test**: Can be fully tested by allocating a DmaBuffer through the IMemoryManagement interface and verifying the returned buffer has the correct size, is non-null, and the allocation is tracked in stats.

**Acceptance Scenarios**:

1. **Given** an initialized SPDK environment with the ISPDKEnv receptacle connected, **When** the developer calls allocate with a valid size and alignment, **Then** a DmaBuffer is returned with the requested size, backed by SPDK hugepage memory.
2. **Given** an initialized SPDK environment, **When** the developer calls allocate with a NUMA node affinity parameter, **Then** the DmaBuffer is allocated from the specified NUMA node's memory.
3. **Given** an initialized SPDK environment, **When** the developer calls allocate without specifying NUMA affinity, **Then** the DmaBuffer is allocated from any available NUMA node.
4. **Given** no SPDK environment connected, **When** the developer calls allocate, **Then** an appropriate error is returned indicating the environment is not initialized.

---

### User Story 2 - Allocate Zero-Initialized DMA Memory (Priority: P1)

A developer needs zero-initialized DMA memory for security-sensitive or protocol-compliant operations where buffer contents must be deterministic. They call zmalloc to get a DMA buffer where all bytes are guaranteed to be zero.

**Why this priority**: Zero-initialized allocation is equally fundamental as regular allocation and is commonly required for storage protocol compliance.

**Independent Test**: Can be fully tested by calling zmalloc and verifying every byte in the returned buffer is zero.

**Acceptance Scenarios**:

1. **Given** an initialized SPDK environment, **When** the developer calls zmalloc with a valid size and alignment, **Then** a DmaBuffer is returned with all bytes initialized to zero.
2. **Given** an initialized SPDK environment, **When** the developer calls zmalloc with a NUMA node affinity parameter, **Then** the zero-initialized DmaBuffer is allocated from the specified NUMA node.

---

### User Story 3 - Free Allocated Memory (Priority: P1)

A developer finishes using a DMA buffer and wants to explicitly return the memory to the SPDK allocator. They call free with the DmaBuffer handle, and the memory is released and allocation stats are updated accordingly.

**Why this priority**: Memory deallocation is essential to prevent resource exhaustion and must work correctly alongside allocation.

**Independent Test**: Can be fully tested by allocating a buffer, freeing it, and verifying that stats reflect the reduced usage.

**Acceptance Scenarios**:

1. **Given** a previously allocated DmaBuffer, **When** the developer calls free with that buffer, **Then** the underlying SPDK memory is released and the allocation stats decrease by the freed amount.
2. **Given** an attempt to track allocations, **When** multiple buffers are allocated and then freed, **Then** the memory stats accurately reflect the current allocation state after each operation.

---

### User Story 4 - Reallocate DMA Memory (Priority: P2)

A developer needs to resize an existing DMA buffer (grow or shrink) while preserving the existing data. They call reallocate with the existing DmaBuffer and a new size. The system allocates new memory, copies the data, frees the old buffer, and returns the new buffer.

**Why this priority**: Reallocation is a common memory management operation but less frequent than initial allocation/deallocation in typical storage workloads.

**Independent Test**: Can be fully tested by allocating a buffer, writing known data, reallocating to a larger size, and verifying the original data is preserved in the new buffer.

**Acceptance Scenarios**:

1. **Given** an existing DmaBuffer of size N, **When** the developer calls reallocate with a larger size M (M > N), **Then** a new DmaBuffer of size M is returned with the first N bytes containing the original data.
2. **Given** an existing DmaBuffer of size N, **When** the developer calls reallocate with a smaller size M (M < N), **Then** a new DmaBuffer of size M is returned with the first M bytes of the original data preserved.
3. **Given** reallocate is called, **When** the allocation succeeds, **Then** the stats are updated to reflect the size change (old allocation removed, new allocation added).

---

### User Story 5 - Query Memory Allocation Statistics (Priority: P2)

A system operator or monitoring component needs visibility into memory usage. They query the component's stats to see total allocated memory, allocation counts broken down by NUMA zone, and remaining available memory.

**Why this priority**: Statistics are essential for operational visibility and capacity planning, but the system functions correctly without them.

**Independent Test**: Can be fully tested by performing a series of allocations and frees, then querying stats and verifying the numbers match the expected state.

**Acceptance Scenarios**:

1. **Given** several allocations across different NUMA nodes, **When** the developer queries stats, **Then** the response includes total bytes allocated, number of active allocations, and a per-NUMA-zone breakdown.
2. **Given** allocations and deallocations have occurred, **When** the developer queries stats, **Then** the stats accurately reflect the current state (not stale data).
3. **Given** the component is freshly created with no allocations, **When** stats are queried, **Then** all counters show zero.

---

### User Story 6 - Thread-Safe Concurrent Access (Priority: P2)

Multiple threads in a storage application need to allocate and free memory concurrently. The component must handle concurrent access safely with lock-based protection of internal stats state, ensuring no data races or corruption.

**Why this priority**: Thread safety is critical for real-world usage in multi-threaded storage applications, but can be validated after basic single-threaded functionality works.

**Independent Test**: Can be fully tested by spawning multiple threads that concurrently allocate and free memory, then verifying stats consistency afterward.

**Acceptance Scenarios**:

1. **Given** multiple threads calling allocate and free concurrently, **When** all operations complete, **Then** the stats are consistent (no negative counts, totals match expected values).
2. **Given** concurrent allocations, **When** a thread queries stats while another thread is allocating, **Then** the stats reflect a consistent snapshot (no torn reads).

---

### Edge Cases

- What happens when allocation is requested but SPDK hugepage memory is exhausted?
- What happens when an alignment of zero is specified?
- What happens when a size of zero is specified?
- What happens when a non-existent NUMA node is specified?
- What happens when the ISPDKEnv receptacle is not connected?
- What happens when reallocate is called with the same size as the original buffer?

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: System MUST expose an IMemoryManagement interface through the Certus component framework using `define_interface!` and `define_component!` macros.
- **FR-002**: System MUST include an ISPDKEnv receptacle to bind to an instantiated SPDK environment component.
- **FR-003**: System MUST provide an `allocate` operation that returns a DmaBuffer backed by SPDK DMA-safe hugepage memory, accepting size, alignment, and an optional NUMA node affinity parameter.
- **FR-004**: System MUST provide a `zmalloc` operation that returns a zero-initialized DmaBuffer, accepting size, alignment, and an optional NUMA node affinity parameter.
- **FR-005**: System MUST provide a `reallocate` operation that resizes an existing DmaBuffer, preserving data up to the minimum of the old and new sizes, accepting the existing buffer, new size, alignment, and an optional NUMA node affinity parameter.
- **FR-006**: System MUST provide a `free` operation that consumes the DmaBuffer (takes ownership), updates internal allocation statistics, and then drops the buffer (triggering its deallocator). This prevents double-free since the caller no longer holds the buffer after calling free.
- **FR-007**: System MUST maintain allocation statistics including: total bytes currently allocated, total number of active allocations, and per-NUMA-zone allocation breakdown (bytes and count per zone). Zones are keyed by NUMA node ID (i32), where -1 represents allocations made without explicit NUMA affinity ("any/unknown").
- **FR-008**: System MUST provide a `stats` query operation that returns a snapshot of current allocation statistics.
- **FR-009**: System MUST use SPDK DMA allocation functions (spdk_dma_malloc, spdk_dma_zmalloc, spdk_dma_free, and spdk_zmalloc/spdk_free for NUMA-pinned allocations) as the underlying memory allocator.
- **FR-010**: System MUST be thread-safe (re-entrant), using lock-based protection for internal stats state to support concurrent allocation and deallocation from multiple threads.
- **FR-011**: System MUST return descriptive errors when the ISPDKEnv receptacle is not connected, when memory is exhausted, when invalid parameters are provided (zero size, zero alignment), or when the SPDK environment is not initialized.
- **FR-012**: System MUST use the DmaBuffer type from the components/interfaces crate as the basis for all memory handles returned to callers.
- **FR-013**: System MUST include unit tests covering all public API correctness, including edge cases.
- **FR-014**: System MUST include Criterion-based performance benchmarks for all performance-sensitive operations (allocate, zmalloc, free, reallocate, stats query).

### Key Entities

- **DmaBuffer**: A DMA-safe memory buffer handle (defined in components/interfaces crate). Represents an allocated region of hugepage memory with pointer, length, NUMA node, free function, and metadata.
- **MemoryAllocatorCpu**: The component implementing IMemoryManagement. Holds an ISPDKEnv receptacle and thread-safe allocation statistics.
- **AllocationStats**: A snapshot of memory allocation state including total bytes allocated, active allocation count, and per-NUMA-zone breakdown. Zones are keyed by NUMA node ID (i32); -1 represents "any/unknown" NUMA affinity.
- **IMemoryManagement**: The interface trait exposing allocate, zmalloc, reallocate, free, and stats operations.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: All allocate, zmalloc, and free operations complete successfully when the SPDK environment is initialized and memory is available.
- **SC-002**: Allocation statistics are always consistent: total allocated bytes equals the sum of all per-zone allocated bytes; active allocation count equals the sum of per-zone counts.
- **SC-003**: Concurrent allocation and deallocation from 8 or more threads produces consistent, non-negative statistics with no data races.
- **SC-004**: Reallocated buffers preserve 100% of the original data (up to the smaller of old and new sizes).
- **SC-005**: All zero-initialized buffers (zmalloc) contain exclusively zero bytes upon return.
- **SC-006**: All public API methods have unit tests achieving full branch coverage of error paths and success paths.
- **SC-007**: Criterion benchmarks exist for allocate, zmalloc, free, reallocate, and stats query operations, enabling regression detection.
- **SC-008**: NUMA-affinity allocations are routed to the specified NUMA node when a node parameter is provided.

## Clarifications

### Session 2026-04-10

- Q: Should stats zone keys be NUMA node IDs, named strings, or caller-defined tags? → A: Zones are NUMA node IDs (i32), including -1 for "any/unknown".
- Q: Should `free` consume the DmaBuffer or rely on Drop for deallocation? → A: `free` consumes the DmaBuffer (takes ownership), updates stats, then drops it.

## Assumptions

- The SPDK environment (spdk-env component) is initialized before any memory allocation operations are called on this component.
- The host system has hugepages configured for SPDK/DPDK memory allocation.
- The DmaBuffer type from the interfaces crate is the canonical type for representing DMA-safe memory handles in the Certus system.
- The component framework macros (define_interface!, define_component!) support the patterns needed for receptacles and interface exposure, consistent with existing components (spdk-env, spdk-simple-block-device).
- SPDK DMA allocation functions are available via the spdk-sys crate's FFI bindings.
- Stats tracking overhead (lock acquisition/release) is acceptable for the target workloads; the lock protects only the stats counters, not the SPDK allocation calls themselves.
- Unit tests for allocation/free correctness can use mock or stub approaches for SPDK functions, since actual SPDK initialization requires hardware and hugepage setup.
- Criterion benchmarks that exercise real SPDK allocation will require an integration test environment with hugepages; benchmark structure and harness should be in place regardless.
