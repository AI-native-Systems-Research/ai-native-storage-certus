# Feature Specification: SSD Integration Test

**Feature Branch**: `002-ssd-integration-test`

**Created**: 2026-07-01

**Status**: Draft

**Input**: User description: "Build a test that uses the real SSD and accesses partition 1 according to a disk-partition-manager instance. The test should validate the operations and data integrity."

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Validate Put and Get on Real Hardware (Priority: P1)

A developer runs the integration test against a real SSD to confirm that key-value pairs written via `put` can be correctly retrieved via `get`, exercising the full I/O path through IBlockDevice and IPartitionTable with partition 1.

**Why this priority**: The fundamental store/retrieve path must work on real hardware before any other operations can be trusted. This validates the entire stack end-to-end.

**Independent Test**: Can be fully tested by writing known key-value pairs to partition 1, reading them back, and comparing bytes. Delivers confidence that the on-disk layout and block device I/O are correct.

**Acceptance Scenarios**:

1. **Given** a real SSD with partition 1 managed by disk-partition-manager, **When** the test writes a key-value pair via `put` and reads it back via `get`, **Then** the retrieved value matches the original bytes exactly.
2. **Given** multiple key-value pairs of varying sizes (1 byte, 4KiB, 128KiB max), **When** each is stored and retrieved, **Then** all values are byte-for-byte correct.
3. **Given** a key that has not been written, **When** `get` is called, **Then** the result indicates not-found.

---

### User Story 2 - Validate Delete Operations (Priority: P2)

A developer confirms that deleted entries are no longer retrievable and do not appear during iteration, verifying that delete operations correctly update both in-memory state and on-disk persistence.

**Why this priority**: Delete correctness is critical for data lifecycle management but depends on put/get working first.

**Independent Test**: Can be tested by writing entries, deleting them, then verifying `get` returns not-found and `iterate_all` excludes them.

**Acceptance Scenarios**:

1. **Given** a stored entry at key K, **When** `delete(K)` is called followed by `get(K)`, **Then** the result is not-found.
2. **Given** a stored entry at key K, **When** `delete(K)` is called and then `iterate_all` is invoked, **Then** key K does not appear in the iteration results.

---

### User Story 3 - Validate Data Persistence Across Restart (Priority: P1)

A developer confirms that data written to the SSD survives a component restart. After force-flushing and re-initializing the store from the same partition, all previously flushed entries are intact.

**Why this priority**: Persistence is the core value proposition. If data doesn't survive restarts on real hardware, the component is non-functional.

**Independent Test**: Can be tested by writing entries, calling `force_flush`, tearing down and re-creating the component instance, then verifying all entries are retrievable.

**Acceptance Scenarios**:

1. **Given** entries written and `force_flush()` called, **When** the store component is torn down and re-initialized on the same partition, **Then** all flushed entries are retrievable with correct values.
2. **Given** entries written without calling `force_flush()`, **When** the store is re-initialized, **Then** it is acceptable for unflushed entries to be absent (no corruption observed).

---

### User Story 4 - Validate Iterate All on Real Data (Priority: P2)

A developer confirms that `iterate_all` correctly enumerates all stored entries from real SSD-backed storage, with no missing or duplicate entries.

**Why this priority**: Iteration correctness is important for administrative operations but depends on put/get working first.

**Independent Test**: Can be tested by inserting a known set of entries, calling `iterate_all`, and verifying the exact set is returned (no extras, no missing).

**Acceptance Scenarios**:

1. **Given** N entries stored on the real SSD, **When** `iterate_all()` is called, **Then** exactly N entries are visited, each matching the stored key-value pairs.
2. **Given** an empty store (freshly initialized partition), **When** `iterate_all()` is called, **Then** zero entries are returned.

---

### User Story 5 - Validate Data Integrity Under Load (Priority: P2)

A developer confirms that writing a large number of entries does not corrupt previously written data. The test exercises capacity limits and verifies that reads after bulk writes return correct data.

**Why this priority**: Validates that the on-disk layout handles realistic workloads without corruption or silent data loss.

**Independent Test**: Can be tested by writing hundreds of entries, then reading back every single one and comparing bytes.

**Acceptance Scenarios**:

1. **Given** 500+ entries written sequentially, **When** each entry is read back, **Then** 100% of values match their original content.
2. **Given** entries written up to near-capacity, **When** the store reports capacity exhaustion, **Then** all previously written entries remain retrievable and intact.

---

### Edge Cases

- What happens when the test writes the maximum value size (128KiB) to partition 1? The full value must be correctly stored and retrieved.
- What happens if partition 1 is too small for the test workload? The test must detect capacity errors and report them clearly rather than hanging or corrupting data.
- What happens when reading from a freshly zeroed partition (no prior store data)? The store should initialize cleanly with zero entries.
- What happens when the test overwrites an existing key with a different-sized value? The new value must replace the old one completely.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: Test MUST use a real SSD device (not a mock or in-memory substitute) for all I/O operations.
- **FR-002**: Test MUST access partition 1 as determined by a disk-partition-manager component instance.
- **FR-003**: Test MUST validate `put` operations by storing entries and confirming success.
- **FR-004**: Test MUST validate `get` operations by retrieving stored entries and comparing byte-for-byte against original data.
- **FR-005**: Test MUST validate `delete` operations by confirming deleted keys are no longer retrievable.
- **FR-006**: Test MUST validate `iterate_all` by confirming the complete set of stored entries is enumerable.
- **FR-007**: Test MUST validate persistence by re-initializing the store after `force_flush` and confirming entries survive.
- **FR-008**: Test MUST validate data integrity by exercising varying value sizes (small, medium, maximum 128KiB).
- **FR-009**: Test MUST validate that overwriting an existing key replaces the value completely.
- **FR-010**: Test MUST validate bulk workload integrity by writing hundreds of entries and reading all back.
- **FR-011**: Test MUST use the component's standard IExtendedMetadataStore interface (not internal APIs).
- **FR-012**: Test MUST report clear pass/fail results for each validation category.

### Key Entities

- **SSD Device**: The real NVMe or SATA device providing the physical storage medium.
- **Partition 1**: The disk partition (as reported by disk-partition-manager) allocated for the metadata store during testing.
- **Test Entry**: A key-value pair with known content used for validation (key: u64, value: byte blob).
- **Disk Partition Manager**: The component instance that provides partition table information for partition 1.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: 100% of stored entries are retrievable with byte-for-byte correctness after put+get round-trip.
- **SC-002**: 100% of flushed entries survive component restart on the same partition.
- **SC-003**: Zero data corruption or partial entries observed across all test scenarios.
- **SC-004**: Delete operations result in 100% of targeted keys being non-retrievable and non-iterable.
- **SC-005**: Bulk write of 500+ entries completes without any integrity failures.
- **SC-006**: Test exercises at least 3 distinct value sizes (minimum, typical, and maximum 128KiB).

## Assumptions

- A real SSD device is available on the test machine with at least one partition managed by disk-partition-manager.
- Partition 1 is available and can be exclusively used by the test (no concurrent access from other components during testing).
- The disk-partition-manager component is functional and correctly reports partition 1 boundaries.
- The extended-metadata-store component is built and its IExtendedMetadataStore interface is available.
- The test may destructively write to partition 1 — any existing data on that partition may be overwritten.
- The test environment has sufficient permissions to perform direct device I/O (e.g., hugepages, IOMMU configured if using SPDK).
- The IBlockDevice receptacle is wired to a real NVMe or SATA block device driver (not a mock).
