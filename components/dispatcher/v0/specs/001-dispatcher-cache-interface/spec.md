# Feature Specification: Dispatcher Cache Interface

**Feature Branch**: `001-dispatcher-cache-interface`  
**Created**: 2026-04-28  
**Status**: Draft  
**Input**: User description: "Dispatcher component providing IDispatcher interface with cache management methods for GPU-to-SSD data flows"

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Cache Population (GPU to SSD) (Priority: P1)

A client application holds data in GPU memory and wants to cache it for future use. The client calls the dispatcher's populate method, providing a cache key and an IPC handle referencing the GPU memory region. The dispatcher registers the element in the dispatch map, allocates a CPU staging buffer, initiates a DMA copy from GPU memory into the staging buffer, and returns confirmation to the client. In the background, the dispatcher asynchronously writes the staging buffer contents to the SSD via the block device and extent manager, then frees the staging buffer.

**Why this priority**: This is the primary write path — without the ability to populate the cache, no data enters the system. Every other operation depends on cached data existing.

**Independent Test**: Can be fully tested by populating a cache entry with a known key and verifying that the dispatch map contains the entry and that the data eventually reaches the block device. Delivers the core caching capability.

**Acceptance Scenarios**:

1. **Given** the dispatcher is initialized with all receptacles bound, **When** populate(key, ipc_handle) is called with a new key, **Then** a staging buffer is allocated, DMA copy from GPU is initiated, and the call returns success before the SSD write completes.
2. **Given** a populate call returned successfully, **When** the background SSD write completes, **Then** the dispatch map entry transitions from staging to block-device state and the staging buffer is freed.
3. **Given** the dispatcher is initialized, **When** populate(key, ipc_handle) is called with a key that already exists, **Then** an appropriate error is returned indicating duplicate key.

---

### User Story 2 - Cache Lookup with DMA Transfer (Priority: P1)

A client application needs to retrieve previously cached data into GPU memory. The client calls the dispatcher's lookup method, providing the cache key and an IPC handle for the destination GPU memory. The dispatcher queries the dispatch map; if the data is in a staging buffer, it initiates a DMA copy from the staging buffer to GPU memory. If the data is on SSD, it reads from the block device and transfers to GPU memory. The client receives the data.

**Why this priority**: This is the primary read path. The cache is only useful if data can be retrieved. Lookup is the most latency-sensitive operation.

**Independent Test**: Can be tested by first populating a cache entry, then looking it up and verifying the DMA transfer to the client's memory occurs with correct data.

**Acceptance Scenarios**:

1. **Given** a cache entry exists in staging state, **When** lookup(key, ipc_handle) is called, **Then** a DMA copy from the staging buffer to the GPU memory region is performed and success is returned.
2. **Given** a cache entry exists in block-device state (SSD), **When** lookup(key, ipc_handle) is called, **Then** data is read from the SSD at the recorded offset and DMA-copied to the GPU memory region.
3. **Given** no cache entry exists for the key, **When** lookup(key, ipc_handle) is called, **Then** a cache-miss indication is returned.

---

### User Story 3 - Cache Presence Check (Priority: P2)

A client application wants to check whether a cache entry exists without transferring any data. The client calls the dispatcher's check method with a cache key. The dispatcher queries the dispatch map and returns whether the key is present.

**Why this priority**: Enables clients to make decisions about whether to populate or look up data without incurring DMA transfer costs. Important for efficiency but not required for basic functionality.

**Independent Test**: Can be tested by checking a non-existent key (returns not present), populating a key, then checking again (returns present).

**Acceptance Scenarios**:

1. **Given** a cache entry exists for the key, **When** check(key) is called, **Then** the result indicates the entry is present.
2. **Given** no cache entry exists for the key, **When** check(key) is called, **Then** the result indicates the entry is not present.

---

### User Story 4 - Cache Entry Removal (Priority: P2)

A client application wants to evict a cache entry. The client calls the dispatcher's remove method with a cache key. The dispatcher frees the associated staging buffer (if data has not yet been written to SSD) or frees the extent on the SSD (if data has been committed). The dispatch map entry is removed.

**Why this priority**: Cache eviction is necessary for cache management and preventing resource exhaustion. Required for long-running workloads but not for basic single-use caching.

**Independent Test**: Can be tested by populating an entry, removing it, then verifying the key is no longer present and resources have been freed.

**Acceptance Scenarios**:

1. **Given** a cache entry exists in staging state, **When** remove(key) is called, **Then** the staging buffer is freed and the dispatch map entry is removed.
2. **Given** a cache entry exists in block-device state, **When** remove(key) is called, **Then** the extent is freed via the extent manager and the dispatch map entry is removed.
3. **Given** no cache entry exists for the key, **When** remove(key) is called, **Then** an appropriate error is returned.

---

### User Story 5 - Dispatcher Initialization and Wiring (Priority: P1)

A system integrator wires the dispatcher component to its dependencies: a logger, N+1 block devices (1 metadata + N data), and N extent managers. The integrator provides PCI addresses for the metadata and data block devices. The dispatcher initializes each extent manager with its corresponding metadata partition and data block device size. After initialization, the dispatcher is ready to serve cache operations.

**Why this priority**: Without correct initialization and wiring, no cache operations can proceed. This is the prerequisite for all other stories.

**Independent Test**: Can be tested by wiring all receptacles and calling initialize, verifying that the dispatcher transitions to an operational state and that extent managers are correctly configured.

**Acceptance Scenarios**:

1. **Given** the dispatcher component is created, **When** a logger, N+1 block devices, and N extent managers are bound to their receptacles, **Then** initialize succeeds and the dispatcher is ready for cache operations.
2. **Given** the dispatcher component is created, **When** initialize is called without all required receptacles bound, **Then** an error is returned indicating which dependency is missing.
3. **Given** initialization succeeds, **When** shutdown is called, **Then** all resources are released and background operations complete or are cancelled.

---

### Edge Cases

- When DMA buffer allocation fails during populate (out of memory), populate returns an allocation failure error to the caller and no dispatch map entry is created.
- When a populate is in progress (staging phase) and a lookup is called for the same key, the lookup blocks until the populate releases its write reference (per dispatch map read/write locking semantics), then serves from the staging buffer or SSD depending on current state.
- When the SSD is full and a background write cannot allocate an extent, an error is raised, the entry is removed from the dispatch map, and the staging buffer is freed.
- When remove is called while a background SSD write is in progress, the remove blocks until the write completes (or fails), then removes the entry and frees all resources (staging buffer and/or SSD extent).
- Multiple concurrent lookups for the same key are permitted (multiple read references allowed by dispatch map locking semantics).
- When the block device reports an I/O error during a background write, an error is raised, the entry is removed from the dispatch map, and the staging buffer is freed (same handling as SSD-full).

## Clarifications

### Session 2026-04-28

- Q: Are cache entries fixed-size or variable-size, and what bounds apply? → A: Variable-size entries, bounded by the extent manager's configured max extent size (default 1 GiB).
- Q: What happens when the SSD is full during a background write from staging? → A: An error is raised. The entry is removed from the dispatch map and the staging buffer is freed.
- Q: What happens when the block device reports an I/O error during a background write? → A: Same as SSD-full — raise error, remove entry from dispatch map, free staging buffer.
- Q: What happens when remove is called during an in-flight background write? → A: Remove blocks until the background write completes (or fails), then removes the entry and frees all resources.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: System MUST define an `IDispatcher` interface in the shared interfaces crate, providing `lookup`, `check`, `remove`, `populate`, `initialize`, and `shutdown` methods.
- **FR-002**: System MUST define a `DispatcherError` error type in the shared interfaces crate, covering all failure modes (not initialized, key not found, duplicate key, I/O error, allocation failure, timeout).
- **FR-003**: The `populate(key, ipc_handle)` method MUST register the element in the dispatch map, allocate a variable-size DMA staging buffer (bounded by the extent manager's max extent size), initiate DMA copy from the client's GPU memory into the staging buffer, and return confirmation to the caller.
- **FR-004**: After a successful populate, the system MUST asynchronously write the staging buffer contents to the SSD via the block device and extent manager, transitioning the dispatch map entry from staging to block-device state.
- **FR-005**: The staging buffer MUST be freed after the asynchronous SSD write completes successfully.
- **FR-006**: The `lookup(key, ipc_handle)` method MUST query the dispatch map; if the data is in staging, perform DMA copy from staging buffer to the client's GPU memory; if on SSD, read from block device and DMA-copy to the client's GPU memory.
- **FR-007**: The `lookup` method MUST return a cache-miss indication if the key does not exist in the dispatch map.
- **FR-008**: The `check(key)` method MUST return whether a cache entry exists for the given key without performing any data transfer.
- **FR-009**: The `remove(key)` method MUST free the staging buffer (if data is in staging state) or free the extent on SSD (if data is in block-device state) and remove the dispatch map entry.
- **FR-010**: The dispatcher component MUST use the component framework's `define_component!` macro and expose only the `IDispatcher` interface.
- **FR-011**: The dispatcher MUST accept receptacles for `ILogger`, `IBlockDeviceAdmin`, and `IDispatchMap` components.
- **FR-012**: The `initialize` method MUST validate that all required receptacles are bound before proceeding.
- **FR-013**: The dispatcher MUST use appropriate read/write locking on the dispatch map to ensure thread safety during concurrent operations.
- **FR-014**: The `shutdown` method MUST ensure all in-flight background operations complete or are cancelled before returning.
- **FR-015**: The dispatcher MUST coordinate N data block devices with N extent managers, where each extent manager is associated with a specific metadata partition and data block device.
- **FR-016**: The dispatcher MUST pass the data block device size and a unique identifier (derived from the controller PCI address) to each extent manager's format function.
- **FR-017**: When the asynchronous background write fails (extent allocation failure or block device I/O error), the dispatcher MUST raise an error, remove the entry from the dispatch map, and free the staging buffer.
- **FR-018**: When `remove(key)` is called while a background write is in progress for that key, the remove MUST block until the background write completes (or fails), then remove the entry and free all associated resources.
- **FR-019**: All block device I/O operations MUST be segmented to respect the device's Maximum Data Transfer Size (MDTS, typically 128 KiB). Reads and writes larger than MDTS MUST be split into multiple sequential or batched I/O operations.

### Key Entities

- **CacheKey**: A 64-bit identifier for cached data elements. Used to address entries in the dispatch map.
- **IPC Handle**: An opaque reference to a GPU memory region provided by the client for DMA transfers.
- **Staging Buffer**: A CPU-accessible DMA buffer used as an intermediate store between GPU memory and SSD storage. Variable-size, bounded by the extent manager's max extent size.
- **Dispatch Map Entry**: A record tracking the state of a cached element — whether it is in staging (CPU buffer) or committed to a block device (SSD offset).
- **Extent**: A contiguous region on a data block device, managed by the extent manager, used to store committed cache data.
- **Data Block Device**: An NVMe SSD that holds cached data. There are N data block devices in the system.
- **Metadata Block Device**: An NVMe SSD with partitions (namespaces) that holds metadata for the extent managers.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A client can populate a cache entry and subsequently retrieve it via lookup, receiving the correct data, within a single session.
- **SC-002**: Cache check operations return accurate presence information for both existing and non-existing keys.
- **SC-003**: Cache removal frees all associated resources (staging buffers and SSD extents) so that they can be reused.
- **SC-004**: The dispatcher correctly handles concurrent populate and lookup operations on different keys without data corruption or deadlock.
- **SC-005**: Initialization fails gracefully with a descriptive error when required dependencies are not bound.
- **SC-006**: Shutdown completes all in-flight background writes before returning, ensuring no data loss.
- **SC-007**: The dispatcher supports N independent data block devices and extent managers operating in parallel.

## Assumptions

- Clients provide valid IPC handles referencing accessible GPU memory regions. The dispatcher does not validate GPU memory accessibility.
- The SPDK environment is initialized and active before the dispatcher component is created.
- DMA buffer allocation uses the SPDK DMA allocator (via `DmaBuffer::new`), not a custom allocator function.
- A fixed timeout of 100ms is used for blocking operations. Variable per-call timeouts are not supported.
- The block device identifier for extent manager association is derived implicitly from the controller's PCI address; callers do not need to specify it explicitly.
- The `IDispatcher` interface does not need to expose extent manager or block device admin configuration — these are wired via receptacles and initialized internally.
- GPU-to-CPU and CPU-to-GPU DMA transfers are handled by the system's DMA engine; the dispatcher orchestrates but does not implement the transfer mechanism.
- NVMe SSDs have a Maximum Data Transfer Size (MDTS) limit, typically 128 KiB. The dispatcher must query this from the block device and segment I/O accordingly.
