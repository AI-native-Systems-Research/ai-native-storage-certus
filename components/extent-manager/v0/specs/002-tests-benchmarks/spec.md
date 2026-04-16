# Feature Specification: Tests and Benchmarks

**Feature Branch**: `002-tests-benchmarks`
**Created**: 2026-04-15
**Status**: Draft
**Input**: User description: "Build unit tests to check API operation and data integrity in simulated power-failure. Include tests for thread-safety. Implement benchmarks for basic interface operations."

## User Scenarios & Testing *(mandatory)*

### User Story 1 - API Operation Tests (Priority: P1)

A developer runs the test suite and verifies that every public operation on the extent manager works correctly through a mock block device — without requiring real NVMe hardware or SPDK infrastructure.

**Why this priority**: The component currently has zero tests for its core `IExtentManager` operations (`create_extent`, `remove_extent`, `lookup_extent`, `extent_count`), the `initialize`/`open` lifecycle, and the `recovery::recover` path. These are the most critical gaps against the project constitution.

**Independent Test**: Can be fully tested by running `cargo test -p extent-manager` and verifying all API operations return correct results through a mock block device. Delivers confidence that the core CRUD and lifecycle operations behave correctly.

**Acceptance Scenarios**:

1. **Given** a freshly initialized extent manager (via mock block device), **When** a developer creates an extent with a unique key, **Then** the extent count increases by one and `lookup_extent` returns matching metadata.
2. **Given** an extent manager with one extent, **When** the developer removes that extent, **Then** the extent count returns to zero and `lookup_extent` returns a "key not found" error.
3. **Given** an extent manager with multiple size classes, **When** extents are created across all classes until each class is full, **Then** subsequent creates for full classes return an "out of space" error.
4. **Given** a developer creates an extent with a key that already exists, **When** the duplicate create is attempted, **Then** a "duplicate key" error is returned.
5. **Given** an extent manager has been initialized and populated, **When** the developer re-opens it (simulating restart), **Then** all previously created extents are recovered and accessible via lookup.

---

### User Story 2 - Simulated Power-Failure Data Integrity (Priority: P2)

A developer verifies that the extent manager's crash-consistency guarantees hold under simulated power-failure conditions — specifically that interrupted writes leave the system in a recoverable state.

**Why this priority**: The extent manager's two-phase write protocol (record write → bitmap flip) is a core correctness property. If power fails between these steps, recovery must detect and clean orphan records. Testing this without real hardware is essential for CI.

**Independent Test**: Can be fully tested by injecting failures at specific points in the mock block device (e.g., failing writes after N operations, simulating partial sequences) and verifying that `open()` recovery produces correct results.

**Acceptance Scenarios**:

1. **Given** a create operation where the record write succeeds but the bitmap persist fails (simulated power-failure mid-create), **When** the developer re-opens the extent manager, **Then** recovery detects the orphan record and cleans it up, reporting it in `RecoveryResult.orphans_cleaned`.
2. **Given** a remove operation where the bitmap clear fails after success (simulated power-failure mid-remove), **When** the developer re-opens the extent manager, **Then** the extent remains present (as if the remove never happened) and the system is consistent.
3. **Given** a series of successful creates followed by an abrupt stop (no graceful shutdown), **When** the developer re-opens, **Then** all successfully committed extents are recovered with correct metadata and the extent count matches expectations.

---

### User Story 3 - Thread Safety (Priority: P3)

A developer verifies that the extent manager handles concurrent access from multiple threads without data corruption, deadlocks, or lost updates.

**Why this priority**: The component uses interior mutability (`RwLock`, `Mutex`) for thread safety, but this has not been validated under concurrent load. Thread-safety bugs are notoriously hard to reproduce and must be tested explicitly.

**Independent Test**: Can be fully tested by spawning multiple threads that concurrently create, remove, and look up extents on a shared extent manager instance, and verifying final state consistency.

**Acceptance Scenarios**:

1. **Given** an initialized extent manager shared across multiple threads, **When** all threads concurrently create extents with unique keys, **Then** every create succeeds, the total extent count equals the number of creates, and all extents are individually retrievable.
2. **Given** an initialized extent manager with extents, **When** some threads create new extents while others remove existing ones concurrently, **Then** the final state is consistent — no phantom extents, no lost removes, count matches (creates - removes).
3. **Given** an initialized extent manager, **When** many threads perform concurrent lookups on existing keys, **Then** all lookups return correct metadata and no thread blocks indefinitely (no deadlocks).

---

### User Story 4 - Performance Benchmarks (Priority: P4)

A developer runs benchmarks to establish performance baselines for core operations, enabling detection of regressions and informing optimization decisions.

**Why this priority**: The project constitution mandates Criterion benchmarks for performance-sensitive code. Benchmarks are required for CI gate (`cargo bench --no-run` must compile). This story provides baseline metrics but does not block functional correctness.

**Independent Test**: Can be fully tested by running `cargo bench -p extent-manager` and verifying that benchmark results are produced for each core operation.

**Acceptance Scenarios**:

1. **Given** the extent manager test infrastructure is in place, **When** a developer runs benchmarks, **Then** throughput and latency results are reported for create, lookup, remove, and extent_count operations.
2. **Given** benchmarks exist, **When** a developer runs `cargo bench --no-run`, **Then** all benchmarks compile successfully (CI gate requirement).

---

### Edge Cases

- What happens when `initialize()` is called on a device too small to hold the metadata? The system must return a clear error.
- What happens when `open()` reads a superblock with corrupted magic or checksum? The system must return a corruption error, not panic.
- What happens when `create_extent` is called before `initialize()` or `open()`? The system must return a "not initialized" error.
- What happens when all slots in every size class are exhausted? The system must return an "out of space" error for each full class.
- What happens when `remove_extent` is called with a key that does not exist? The system must return a "key not found" error.
- What happens when `lookup_extent` is called with a key that was removed? The system must return a "key not found" error.
- What happens when metadata serialization round-trips through `to_bytes`/`from_bytes`? All fields must be preserved exactly, including optional filename and CRC.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The test suite MUST include a mock block device that simulates `IBlockDevice` behavior using in-memory storage, without requiring SPDK, hugepages, or NVMe hardware.
- **FR-002**: The mock block device MUST support fault injection — the ability to make specific write operations fail on demand to simulate power-failure scenarios.
- **FR-003**: Unit tests MUST cover all four `IExtentManager` operations: `create_extent`, `remove_extent`, `lookup_extent`, and `extent_count`.
- **FR-004**: Unit tests MUST cover the full component lifecycle: `initialize` on a fresh device and `open` on a previously initialized device.
- **FR-005**: Unit tests MUST verify all error paths: duplicate key, key not found, invalid size class, out of space, device too small, uninitialized component, corrupt superblock.
- **FR-006**: Power-failure simulation tests MUST verify the two-phase write protocol: record write followed by bitmap persist. Failure between these steps MUST result in orphan detection during recovery.
- **FR-007**: Power-failure tests MUST verify that recovery produces correct `RecoveryResult` statistics (extents loaded, orphans cleaned, corrupt records).
- **FR-008**: Thread-safety tests MUST verify concurrent creates, concurrent removes, concurrent lookups, and mixed concurrent operations on a shared extent manager instance.
- **FR-009**: Thread-safety tests MUST verify absence of deadlocks by completing within a reasonable timeout.
- **FR-010**: Benchmarks MUST measure throughput for `create_extent`, `remove_extent`, `lookup_extent`, and `extent_count` operations.
- **FR-011**: All tests MUST pass via `cargo test -p extent-manager` without external dependencies.
- **FR-012**: All benchmarks MUST compile via `cargo bench -p extent-manager --no-run` without external dependencies.

### Key Entities

- **Mock Block Device**: An in-memory implementation of `IBlockDevice` that stores blocks in memory and supports configurable fault injection for write operations.
- **Extent**: A fixed-size storage allocation identified by a unique 64-bit key, belonging to a size class, with optional filename and CRC metadata.
- **Recovery Result**: The outcome of crash recovery, reporting extents loaded, orphans cleaned, and corrupt records detected.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: 100% of `IExtentManager` operations have at least one passing test that verifies correct behavior.
- **SC-002**: 100% of `ExtentManagerError` variants have at least one test that triggers that specific error.
- **SC-003**: The test suite includes at least 2 distinct simulated power-failure scenarios that verify orphan detection and data integrity after recovery.
- **SC-004**: Thread-safety tests exercise at least 4 concurrent threads performing mixed operations and verify final state consistency.
- **SC-005**: Benchmarks produce measurable results for all 4 core operations (create, remove, lookup, count).
- **SC-006**: `cargo test -p extent-manager` passes with zero failures and zero panics.
- **SC-007**: `cargo bench -p extent-manager --no-run` compiles successfully.

## Assumptions

- Tests use an in-memory mock of `IBlockDevice` — no real NVMe hardware, SPDK environment, or hugepages are required.
- The mock block device uses the component framework's existing channel primitives (`SpscChannel`, `Sender`, `Receiver`) and `DmaBuffer::from_raw` (or an equivalent memory-safe substitute) to avoid SPDK-linked DMA allocation.
- Power-failure simulation is achieved by injecting failures into mock block device write operations at controlled points, not by actually interrupting the process.
- Thread-safety tests use standard thread spawning with shared `Arc` references, not async runtimes.
- Benchmarks target the mock block device (measuring algorithmic/overhead costs), not real NVMe I/O latency.
- The existing `criterion 0.5` dev-dependency is used for benchmarks.
- The mock block device is scoped to test infrastructure only — it is not a general-purpose `IBlockDevice` provider and is not intended for use outside of the extent-manager test suite.
