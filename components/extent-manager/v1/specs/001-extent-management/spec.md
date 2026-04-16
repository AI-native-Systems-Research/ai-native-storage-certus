# Feature Specification: Extent Management

**Feature Branch**: `001-extent-management`  
**Created**: 2026-04-16  
**Status**: Draft  
**Input**: User description: "Extent Manager component managing fixed-size storage extents on NVMe SSDs with crash-consistent metadata"

## Clarifications

### Session 2026-04-16

- Q: How should the system handle block device I/O errors during metadata operations? → A: Propagate the error immediately to the caller with no retry and no rollback attempt.
- Q: What is the maximum length for the optional filename field? → A: 255 bytes (POSIX NAME_MAX standard).
- Q: What consistency guarantee should iteration provide during concurrent modification? → A: Exclusive lock — iteration blocks all concurrent modifications until complete.
- Q: What is the expected maximum number of slots per size class? → A: 10,000,000 (10 million) slots per size class.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Create and Allocate Extents (Priority: P1)

A storage client needs to allocate a new fixed-size extent on disk. The client provides a unique 64-bit key, a desired extent size, and optionally a filename and data CRC. The system allocates contiguous disk space, records the extent metadata, and returns confirmation so the client can begin writing data at the allocated location.

**Why this priority**: Extent creation is the fundamental operation — without it, no data can be stored. All other operations depend on extents existing.

**Independent Test**: Can be fully tested by creating an extent with a key and size, then verifying the extent exists with correct metadata. Delivers the core value of space allocation and metadata tracking.

**Acceptance Scenarios**:

1. **Given** an initialized extent manager with available space, **When** a client creates an extent with key=42, size=128KiB, **Then** the system allocates contiguous space, persists metadata, and returns the extent's on-disk location (namespace, offset).
2. **Given** an initialized extent manager, **When** a client creates an extent with key=42, size=128KiB, filename="data.bin", crc=0xABCD1234, **Then** all optional metadata fields are stored and retrievable.
3. **Given** an extent manager with no remaining space for the requested size, **When** a client attempts to create an extent, **Then** the system returns an appropriate error indicating insufficient space.
4. **Given** an extent manager, **When** a client creates an extent with a key that already exists, **Then** the system returns an appropriate error indicating duplicate key.

---

### User Story 2 - Look Up and Read Extent Metadata (Priority: P1)

A storage client needs to locate an extent by its unique key to read the associated data from the block device. The system returns the extent's metadata including on-disk location, namespace ID, offset, and any optional fields (filename, CRC).

**Why this priority**: Lookup is equally fundamental to creation — clients must be able to find their data to read it. This is the read-path complement to creation.

**Independent Test**: Can be tested by creating an extent, then looking it up by key and verifying all metadata fields match.

**Acceptance Scenarios**:

1. **Given** an extent manager with an extent stored under key=42, **When** a client looks up key=42, **Then** the system returns the extent's namespace ID, offset, size, and any optional metadata.
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

---

### User Story 6 - Concurrent Access (Priority: P2)

Multiple threads simultaneously create, remove, and look up extents. The system handles all operations correctly without data races, corruption, or deadlocks.

**Why this priority**: Thread safety is required for production use where multiple clients operate concurrently, but correctness of individual operations must be established first.

**Independent Test**: Can be tested by running multiple threads performing mixed create/remove/lookup operations simultaneously and verifying all operations complete correctly with no data races or inconsistencies.

**Acceptance Scenarios**:

1. **Given** multiple threads creating extents concurrently, **When** all operations complete, **Then** every extent is correctly allocated with unique space, and no two extents overlap.
2. **Given** multiple threads creating and removing extents concurrently, **When** all operations complete, **Then** the extent manager state is consistent — no space leaks, no phantom extents.

---

### User Story 7 - Initialization with Multiple Size Classes (Priority: P1)

A storage system configures the extent manager at startup with a set of supported extent sizes (1 to 32 sizes, ranging from 128KiB to 5MiB, all multiples of 4KiB) and the number of slots per size class. The system formats the underlying storage, writes management structures, and is ready for extent operations.

**Why this priority**: Initialization is the entry point — the system cannot be used without it. Supporting configurable size classes is core to the design.

**Independent Test**: Can be tested by initializing the manager with various size/slot configurations, then verifying the system accepts creates for each configured size and rejects unsupported sizes.

**Acceptance Scenarios**:

1. **Given** a block device and configuration specifying 3 size classes (128KiB, 512KiB, 2MiB) with slot counts, **When** the manager is initialized, **Then** management structures are written and the system is ready for operations.
2. **Given** an initialized manager, **When** a client creates an extent of an unsupported size, **Then** the system returns an appropriate error.
3. **Given** a previously initialized block device, **When** the manager is opened (not initialized), **Then** existing metadata is loaded and all previously stored extents are accessible.

---

### Edge Cases

- What happens when all slots for a particular size class are exhausted but other sizes have space?
- When the block device returns an I/O error during metadata persistence, the error is propagated immediately to the caller with no retry and no rollback attempt.
- What happens when the system is initialized with the maximum number of size classes (32)?
- What happens when the system is initialized with the minimum (1 size class, 1 slot)?
- How does the system handle an attempt to open an uninitialized or corrupted block device?
- During iteration, all concurrent create and remove operations are blocked until iteration completes (exclusive lock model).

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: System MUST allow creation of extents given a unique 64-bit key, an extent size, and optional filename (max 255 bytes) and CRC metadata.
- **FR-002**: System MUST allocate contiguous disk space from the appropriate size class when creating an extent.
- **FR-003**: System MUST persist extent metadata (key, namespace ID, offset, size, optional filename, optional CRC) to the block device.
- **FR-004**: System MUST allow removal of extents by key, freeing the associated disk space for reuse.
- **FR-005**: System MUST allow lookup of extent metadata by key, returning the on-disk location and all stored metadata.
- **FR-006**: System MUST support iterating through all stored extents, visiting each extent exactly once. Iteration MUST hold an exclusive lock that blocks concurrent create and remove operations until iteration completes.
- **FR-007**: System MUST support 1 to 32 fixed extent size classes, configurable at initialization time, ranging from 128KiB to 5MiB, where each size is a multiple of 4KiB. Each size class MUST support up to 10,000,000 slots.
- **FR-008**: System MUST maintain metadata consistency across unexpected power failures using atomic 4KiB writes.
- **FR-009**: System MUST detect and recover from partial writes and metadata corruption on startup.
- **FR-010**: System MUST provide re-entrant, thread-safe access to all operations.
- **FR-011**: System MUST return appropriate errors for: duplicate keys, unknown keys, unsupported sizes, and insufficient space.
- **FR-012**: System MUST support fresh initialization (formatting) and reopening an existing volume.
- **FR-013**: System MUST use a block device receptacle for all persistent storage operations.
- **FR-014**: System MUST use a logger receptacle for all console/diagnostic output.
- **FR-015**: Iteration performance MUST be sufficient for rebuilding in-memory indexes at startup.
- **FR-016**: System MUST propagate block device I/O errors immediately to the caller without retry or rollback attempts.

### Key Entities

- **Extent**: A fixed-size contiguous region of data on disk. Identified by a unique 64-bit key. Contains metadata: namespace ID, on-disk offset (in 4KiB blocks), size class, optional filename (max 255 bytes), optional data CRC.
- **Size Class**: A supported extent size (128KiB to 5MiB, multiples of 4KiB). Each size class has a fixed number of allocation slots. Configured at initialization.
- **Allocation Slot**: A pre-provisioned position within a size class that can hold one extent. Tracks allocated/free state. Up to 10,000,000 slots per size class.
- **Superblock**: The management header stored at a fixed location on the block device. Contains the system configuration (size classes, slot counts) and integrity information.
- **Allocation Bitmap**: A per-size-class structure tracking which slots are allocated and which are free.
- **Extent Record**: A per-slot persistent structure containing the extent's metadata and an integrity check value.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: All extent create, remove, and lookup operations complete successfully under normal conditions with zero data loss.
- **SC-002**: After any simulated power failure during create or remove operations, the system recovers to a fully consistent state with no space leaks and no corrupt metadata.
- **SC-003**: Under concurrent access from 8 or more threads performing mixed operations, zero data races, deadlocks, or inconsistencies occur.
- **SC-004**: Iterating all extents visits each extent exactly once and completes in time proportional to the number of stored extents.
- **SC-005**: The system supports at least 32 distinct size classes with up to 10,000,000 slots per class.
- **SC-006**: All operations return meaningful errors for invalid inputs (duplicate keys, missing keys, unsupported sizes, full capacity) rather than crashing or corrupting state.
- **SC-007**: The system can be freshly initialized and reopened an unlimited number of times without metadata degradation.

## Assumptions

- The underlying block device guarantees power-fail atomic writes for 4KiB-aligned, 4KiB-sized writes.
- Extent data (the actual stored content) is managed separately — this component manages only extent metadata and allocation.
- The block device is exclusively owned by this extent manager instance (no concurrent external writers).
- Extent sizes and slot counts are immutable after initialization; reconfiguration requires re-initialization.
- The component-framework provides the wiring infrastructure (receptacles, providers, lifecycle management).
- The logger receptacle is available before extent operations begin.
- A namespace ID identifies the NVMe namespace and is provided at initialization/open time.
