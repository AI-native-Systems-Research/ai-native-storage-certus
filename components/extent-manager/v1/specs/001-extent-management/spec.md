# Feature Specification: Extent Management

**Feature Branch**: `001-extent-management`  
**Created**: 2026-04-16  
**Status**: Revised (drift sync 2026-04-17)  
**Input**: User description: "Extent Manager component managing fixed-size storage extents on NVMe SSDs with crash-consistent metadata"

## Clarifications

### Session 2026-04-16

- Q: How should the system handle block device I/O errors during metadata operations? → A: Propagate the error immediately to the caller with no retry and no rollback attempt.
- Q: What is the maximum length for the optional filename field? → A: 255 bytes (POSIX NAME_MAX standard).
- Q: What consistency guarantee should iteration provide during concurrent modification? → A: Read lock — iteration blocks concurrent create/remove operations but allows concurrent lookups and other iterations.
- Q: What is the expected maximum number of slots per size class? → A: Determined dynamically by slab size and count. A 1 GiB slab provides ~262,000 slots. Multiple slabs per size class are supported (up to 256 total slabs).

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Create and Allocate Extents (Priority: P1)

A storage client needs to allocate a new fixed-size extent on disk. The client provides a unique 64-bit key, a desired extent size, and optionally a filename and data CRC. The system allocates contiguous disk space, records the extent metadata, and returns confirmation so the client can begin writing data at the allocated location.

**Why this priority**: Extent creation is the fundamental operation — without it, no data can be stored. All other operations depend on extents existing.

**Independent Test**: Can be fully tested by creating an extent with a key and size, then verifying the extent exists with correct metadata. Delivers the core value of space allocation and metadata tracking.

**Acceptance Scenarios**:

1. **Given** an initialized extent manager with available space, **When** a client creates an extent with key=42, size=128KiB, **Then** the system allocates contiguous space, persists metadata, and returns the extent's on-disk offset.
2. **Given** an initialized extent manager, **When** a client creates an extent with key=42, size=128KiB, filename="data.bin", crc=0xABCD1234, **Then** all optional metadata fields are stored and retrievable.
3. **Given** an extent manager with no remaining space for the requested size, **When** a client attempts to create an extent, **Then** the system returns an appropriate error indicating insufficient space.
4. **Given** an extent manager, **When** a client creates an extent with a key that already exists, **Then** the system returns an appropriate error indicating duplicate key.

---

### User Story 2 - Look Up and Read Extent Metadata (Priority: P1)

A storage client needs to locate an extent by its unique key to read the associated data from the block device. The system returns the extent's metadata including on-disk offset and any optional fields (filename, CRC).

**Why this priority**: Lookup is equally fundamental to creation — clients must be able to find their data to read it. This is the read-path complement to creation.

**Independent Test**: Can be tested by creating an extent, then looking it up by key and verifying all metadata fields match.

**Acceptance Scenarios**:

1. **Given** an extent manager with an extent stored under key=42, **When** a client looks up key=42, **Then** the system returns the extent's offset, size, and any optional metadata.
2. **Given** an extent manager, **When** a client looks up a key that does not exist, **Then** the system returns an appropriate "not found" error.

---

### User Story 3 - Remove Extents and Free Space (Priority: P2)

A storage client no longer needs an extent and wants to free the associated disk space. The client provides the extent key, and the system removes the metadata and marks the space as available for future allocation.

**Why this priority**: Space reclamation is essential for ongoing operation but is secondary to the ability to create and read extents.

**Independent Test**: Can be tested by creating an extent, removing it by key, verifying it is no longer found, and confirming the space is available for reuse.

**Acceptance Scenarios**:

1. **Given** an extent manager with an extent stored under key=42, **When** a client removes key=42, **Then** the metadata is deleted, space is freed, and subsequent lookups for key=42 return "not found."
2. **Given** an extent manager, **When** a client removes a key that does not exist, **Then** the system returns an appropriate error.
3. **Given** an extent manager where an extent was removed, **When** a client creates a new extent of the same size, **Then** the freed space can be reused.

---

### User Story 4 - Iterate All Extents (Priority: P2)

A storage system needs to rebuild volatile in-memory caches or indexes after startup. It iterates through all extents as fast as possible, receiving each extent's key and metadata.

**Why this priority**: Iteration enables cache/index rebuilds which are critical for system recovery, but only needed at startup or during maintenance — not for every request.

**Independent Test**: Can be tested by creating multiple extents, iterating over all of them, and verifying each extent's metadata is returned exactly once.

**Acceptance Scenarios**:

1. **Given** an extent manager with N extents across multiple size classes, **When** a client iterates all extents, **Then** every extent is visited exactly once with correct metadata.
2. **Given** an empty extent manager, **When** a client iterates all extents, **Then** the iteration completes immediately with zero results.
3. **Given** an extent manager with extents and another thread attempting create/remove, **When** a client is iterating, **Then** the concurrent create/remove operations block until iteration completes.

---

### User Story 5 - Crash Recovery (Priority: P1)

The system experiences an unexpected power failure during extent creation or removal. On restart, the extent manager recovers to a consistent state — no extent metadata is corrupted or partially written. Allocated extents are intact; incomplete operations are cleanly rolled back.

**Why this priority**: Crash consistency is a fundamental correctness requirement. Without it, data loss or corruption occurs on any power failure, making the system unreliable for production use.

**Independent Test**: Can be tested by simulating power failure at various points during create/remove operations, restarting the manager, and verifying all metadata is consistent and no space is leaked.

**Acceptance Scenarios**:

1. **Given** a power failure occurs during extent creation after metadata is written but before the allocation bitmap is updated, **When** the system restarts and the manager is opened, **Then** the orphan record is detected and cleaned up, and the space is available for reuse.
2. **Given** a power failure occurs during extent creation after the bitmap is updated, **When** the system restarts, **Then** the extent is fully intact and accessible by key.
3. **Given** a power failure occurs during extent removal, **When** the system restarts, **Then** the system recovers to a consistent state with no space leaks.
4. **Given** metadata on disk has a corrupted integrity check, **When** the manager is opened, **Then** the corrupt record is detected, cleared from the bitmap, and the space is reclaimed.
5. **Given** a previously initialized block device, **When** the manager is opened, **Then** existing slab table and metadata are loaded and all previously stored extents are accessible.

---

### User Story 6 - Concurrent Access (Priority: P2)

Multiple threads simultaneously create, remove, and look up extents. The system handles all operations correctly without data races, corruption, or deadlocks.

**Why this priority**: Thread safety is required for production use where multiple clients operate concurrently, but correctness of individual operations must be established first.

**Independent Test**: Can be tested by running multiple threads performing mixed create/remove/lookup operations simultaneously and verifying all operations complete correctly with no data races or inconsistencies.

**Acceptance Scenarios**:

1. **Given** multiple threads creating extents concurrently, **When** all operations complete, **Then** every extent is correctly allocated with unique space, and no two extents overlap.
2. **Given** multiple threads creating and removing extents concurrently, **When** all operations complete, **Then** the extent manager state is consistent — no space leaks, no phantom extents.

---

### User Story 7 - Initialization with Slab-Based Allocation (Priority: P1)

A storage system configures the extent manager at startup by specifying the total managed device space and a fixed slab size. The system initializes in-memory state and is ready for operations. Size classes are not pre-declared — the first `create_extent` call for a given size class dynamically allocates a slab on demand.

**Why this priority**: Initialization is the entry point — the system cannot be used without it. Dynamic slab allocation eliminates the need to pre-commit size class configurations.

**Independent Test**: Can be tested by initializing the manager with a total size and slab size, then verifying the system accepts creates for any valid size class and dynamically allocates slabs.

**Acceptance Scenarios**:

1. **Given** a block device and a total size of 100 GiB with 1 GiB slabs, **When** the manager is initialized, **Then** the system is initialized and ready for operations with no pre-allocated slabs.
2. **Given** an initialized manager with no slabs, **When** a client creates the first extent with size=128 KiB, **Then** a new slab is allocated on-demand for that size class.
3. **Given** an initialized manager where a slab for 128 KiB is full, **When** a client creates another 128 KiB extent, **Then** a second slab is allocated for the same size class (auto-grow).
4. **Given** an initialized manager, **When** all slabs are allocated and no space remains, **Then** the system returns an out-of-space error.
5. *(Moved to User Story 5 — Crash Recovery)*

---

### Edge Cases

- What happens when a slab for a size class is full? A new slab for the same size class is allocated automatically if device space remains (auto-grow).
- What happens when all 256 slabs are allocated and no free space remains? The system returns an out-of-space error.
- When the block device returns an I/O error during metadata persistence, the error is propagated immediately to the caller with no retry and no rollback attempt.
- What happens when the system is initialized with a minimal slab size (2 blocks: 1 bitmap + 1 slot)?
- How does the system handle an attempt to open an uninitialized or corrupted block device?
- During iteration, concurrent create and remove operations are blocked until iteration completes. Concurrent lookups and other iterations are allowed (read lock model).
- What happens during a crash while a new slab is being allocated? Slab state is in-memory; if the crash occurs before the bitmap/records are fully written, recovery will detect and clean up partial state.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: System MUST allow creation of extents given a unique 64-bit key, an extent size, and optional filename (max 255 bytes) and CRC metadata.
- **FR-002**: System MUST allocate contiguous disk space from the appropriate size class when creating an extent.
- **FR-003**: System MUST persist extent metadata (key, offset, size, optional filename, optional CRC) to the block device. Each ExtentManager instance manages a single namespace/device — namespace and device identity are not part of the stored metadata.
- **FR-004**: System MUST allow removal of extents by key, freeing the associated disk space for reuse.
- **FR-005**: System MUST allow lookup of extent metadata by key, returning the on-disk location and all stored metadata.
- **FR-006**: System MUST support iterating through all stored extents via `get_extents()`, visiting each extent exactly once. Iteration holds a read lock — concurrent create and remove operations are blocked until iteration completes, but concurrent lookups and iterations are allowed.
- **FR-007**: System MUST support dynamic slab-based allocation. At initialization, the caller specifies total managed space (in bytes) and slab size (in bytes, must be >= 8 KiB and a multiple of 4 KiB). Size classes are NOT pre-declared; the first `create_extent` call for a given size class dynamically allocates a slab. Each slab serves exactly one size class. When a slab is full, a new slab for that class is allocated automatically if space remains. Maximum 256 slabs.
- **FR-008**: System MUST maintain metadata consistency across unexpected power failures using atomic 4KiB writes.
- **FR-009**: System MUST detect and recover from partial writes and metadata corruption on startup.
- **FR-010**: System MUST provide re-entrant, thread-safe access to all operations.
- **FR-011**: System MUST return appropriate errors for: duplicate keys, unknown keys, unsupported sizes, and insufficient space.
- **FR-012**: System MUST support fresh initialization via `initialize(total_size_bytes, slab_size_bytes)`. Reopening an existing volume is handled by crash recovery (see US5/FR-008/FR-009).
- **FR-013**: System MUST use a block device receptacle for all persistent storage operations.
- **FR-014**: System MUST use a logger receptacle for all console/diagnostic output.
- **FR-015**: Iteration performance MUST be sufficient for rebuilding in-memory indexes at startup. The `open()` operation rebuilds the index by scanning all slab bitmaps and reading valid records.
- **FR-016**: System MUST propagate block device I/O errors immediately to the caller without retry or rollback attempts.
- **FR-017**: System MUST accept a DMA allocator via `set_dma_alloc()` before any I/O operations. The allocator is used for all block device buffer allocations.

### Key Entities

- **Extent**: A fixed-size contiguous region of data on disk. Identified by a unique 64-bit key. Contains metadata: on-disk offset (in 4KiB blocks), size class, optional filename (max 255 bytes), optional data CRC.
- **Size Class**: An extent size. Size classes are not pre-declared at initialization; any size can be used in `create_extent` and will trigger slab allocation on demand.
- **Slab**: A contiguous region of device blocks serving exactly one size class. Contains a bitmap region (tracking slot allocation) followed by a record region (one 4 KiB block per slot). Allocated dynamically when a size class needs more capacity. Maximum 256 slabs. Multiple slabs can serve the same size class.
- **Allocation Slot**: A position within a slab that can hold one extent record. Slot count per slab is determined by slab size. Tracks allocated/free state via the slab's bitmap.
- **On-Disk Layout**: Bitmap regions followed by extent record regions. No on-disk superblock — initialization state is held in memory.
- **Allocation Bitmap**: A per-slab structure tracking which slots within that slab are allocated and which are free. One bit per slot, stored in the slab's bitmap region.
- **Extent Record**: A per-slot persistent 4 KiB block containing the extent's metadata and a CRC-32 integrity check at bytes 4092-4096.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: All extent create, remove, and lookup operations complete successfully under normal conditions with zero data loss.
- **SC-002**: After any simulated power failure during create or remove operations, the system recovers to a fully consistent state with no space leaks and no corrupt metadata.
- **SC-003**: Under concurrent access from 8 or more threads performing mixed operations, zero data races, deadlocks, or inconsistencies occur.
- **SC-004**: Iterating all extents visits each extent exactly once and completes in time proportional to the number of stored extents.
- **SC-005**: The system supports up to 256 dynamically allocated slabs across any mix of size classes. Each 1 GiB slab provides approximately 262,000 slots. Total capacity scales with total device size and slab configuration.
- **SC-006**: All operations return meaningful errors for invalid inputs (duplicate keys, missing keys, unsupported sizes, full capacity) rather than crashing or corrupting state.
- **SC-007**: The system can be freshly initialized an unlimited number of times without metadata degradation. Reopening is handled via crash recovery (US5).

## Assumptions

- The underlying block device guarantees power-fail atomic writes for 4KiB-aligned, 4KiB-sized writes.
- Extent data (the actual stored content) is managed separately — this component manages only extent metadata and allocation.
- The block device is exclusively owned by this extent manager instance (no concurrent external writers).
- Slab size is immutable after initialization; new slabs are allocated dynamically as needed. Reconfiguration requires re-initialization.
- The component-framework provides the wiring infrastructure (receptacles, providers, lifecycle management).
- The logger receptacle is available before extent operations begin.
- Each ExtentManager instance manages a single block device namespace.
