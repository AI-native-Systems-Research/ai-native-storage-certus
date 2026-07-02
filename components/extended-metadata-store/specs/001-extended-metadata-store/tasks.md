# Tasks: Extended Metadata Store

**Input**: Design documents from `specs/001-extended-metadata-store/`

**Prerequisites**: plan.md (required), spec.md (required), research.md, data-model.md, contracts/

**Tests**: Unit tests with MockBlockDevice are part of implementation (test_support.rs pattern). Integration tests exist in spec 002.

**Organization**: Tasks are grouped by user story to enable independent implementation and testing.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

---

## Phase 1: Setup

**Purpose**: Project structure and on-disk format definitions

- [x] T001 Define Superblock struct with magic/version/CRC32 serialization in src/on_disk.rs (4096-byte sector-aligned, fields per data-model.md)
- [x] T002 [P] Define EntryRecord struct with key_len/value_len/flags/crc32/key/value serialization in src/on_disk.rs
- [x] T003 [P] Define RegionHeader struct with flush_seq/entry_count/crc32 serialization in src/on_disk.rs
- [x] T004 Implement sector-alignment padding helper functions in src/on_disk.rs (pad_to_sector, bytes_to_sectors)

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: I/O layer and test infrastructure that ALL user stories depend on

**⚠️ CRITICAL**: No user story work can begin until this phase is complete

- [x] T005 Implement BlockDeviceClient struct in src/block_io.rs wrapping ClientChannels with base_lba offset, sector_size, ns_id, and DmaAllocFn
- [x] T006 Implement BlockDeviceClient::read_sectors(lba, count) in src/block_io.rs — allocate DMA buffer, send ReadSync commands, return data
- [x] T007 Implement BlockDeviceClient::write_sectors(lba, data) in src/block_io.rs — allocate DMA buffer, copy data, send WriteSync commands, wait for completion
- [x] T008 [P] Create src/test_support.rs with MockBlockDevice (HashMap<u64, Vec<u8>> storage, 4096-byte blocks, SPSC channel processing thread) — reuse extent-manager pattern
- [x] T009 [P] Implement heap_dma_alloc() in src/test_support.rs (aligned allocation with Layout tracking for dealloc)
- [x] T010 [P] Implement FaultConfig and MockBlockDevice::with_fault_config() in src/test_support.rs for crash simulation (fail_after_n_writes)
- [x] T011 Implement MockBlockDevice::reboot_from(shared_state) in src/test_support.rs for simulating restart with persisted data
- [x] T012 Add IBlockDevice and IPartitionTable receptacles to ExtendedMetadataStoreComponent in src/lib.rs (gated behind `spdk` feature)
- [x] T013 Implement create_test_component() helper in src/test_support.rs that wires MockBlockDevice + MockLogger + heap_dma_alloc and returns component with 128MiB virtual disk

**Checkpoint**: I/O layer and test infrastructure ready — user story implementation can begin

---

## Phase 3: User Story 1 - Store and Retrieve Metadata (Priority: P1) 🎯 MVP

**Goal**: Key-value put/get with in-memory HashMap backed by on-disk persistence

**Independent Test**: put(key, value) + flush + restart + get(key) returns same bytes

### Implementation for User Story 1

- [x] T014 [US1] Upgrade store field from Mutex<HashMap> to RwLock<HashMap<String, Vec<u8>>> in src/lib.rs
- [x] T015 [US1] Add dirty_count: AtomicU64 and flush_seq: AtomicU64 fields to component in src/lib.rs
- [x] T016 [US1] Implement put() to acquire write lock, enforce MAX_VALUE_SIZE, insert entry, increment dirty_count in src/lib.rs
- [x] T017 [US1] Implement get() to acquire read lock, clone value from HashMap in src/lib.rs
- [x] T018 [US1] Implement serialize_region() in src/on_disk.rs — serialize all HashMap entries into a byte buffer (RegionHeader + EntryRecords, sector-aligned)
- [x] T019 [US1] Implement deserialize_region() in src/on_disk.rs — parse region header and entry records from byte buffer, validate CRC per entry, return Vec<(String, Vec<u8>)>
- [x] T020 [US1] Implement write_superblock() and read_superblock() in src/block_io.rs — serialize/deserialize Superblock to/from LBA 0
- [x] T021 [US1] Implement write_region() in src/block_io.rs — write serialized region data to the appropriate region offset (A or B based on active_region toggle)
- [x] T022 [US1] Implement read_region() in src/block_io.rs — read region data from disk at given offset, return raw bytes
- [x] T023 [US1] Implement flush_to_disk() in src/flush.rs — snapshot entries under read lock, serialize to inactive region, write region, update + write superblock, reset dirty_count
- [x] T024 [US1] Add unit test in src/lib.rs: put + get round-trip with MockBlockDevice (varied sizes: 0B, 1B, 4KiB, 128KiB)
- [x] T025 [US1] Add unit test in src/lib.rs: put + flush + verify on-disk data via MockBlockDevice inspection

**Checkpoint**: Core put/get with persistence works — MVP complete

---

## Phase 4: User Story 2 - Persistence Across Restarts (Priority: P1)

**Goal**: Recovery from disk — rebuild in-memory HashMap from on-disk region after restart

**Independent Test**: put + flush + drop + reinit from same MockBlockDevice state + get returns values

### Implementation for User Story 2

- [x] T026 [US2] Implement recover_from_disk() in src/recovery.rs — read superblock, identify active region, read + deserialize region, rebuild HashMap
- [x] T027 [US2] Implement recovery fallback in src/recovery.rs — if active region is corrupt, try inactive region; if both corrupt, return empty store with warning
- [x] T028 [US2] Implement format_fresh() in src/recovery.rs — write empty superblock + empty regions for fresh partition initialization
- [x] T029 [US2] Implement initialize(partition_id) method on component in src/lib.rs — get partition info from IPartitionTable, create BlockDeviceClient with base_lba, call recover_from_disk() or format_fresh()
- [x] T030 [US2] Add unit test: put entries + flush + reboot_from(shared_state) + recover + verify all entries present
- [x] T031 [US2] Add unit test: corrupt active region CRC + recover → falls back to inactive region successfully
- [x] T032 [US2] Add unit test: fresh partition (all zeros) → format_fresh → empty store

**Checkpoint**: Full persistence lifecycle works — store survives restarts

---

## Phase 5: User Story 3 - Delete Metadata Entries (Priority: P2)

**Goal**: Delete removes entries from both in-memory and on-disk (after flush)

**Independent Test**: put + delete + get returns NotFound; delete + flush + restart + iterate_all excludes deleted key

### Implementation for User Story 3

- [x] T033 [US3] Implement delete() to acquire write lock, remove entry, increment dirty_count in src/lib.rs (already done for in-memory — verify works with persistence)
- [x] T034 [US3] Add unit test: put + delete + flush + reboot + get returns NotFound
- [x] T035 [US3] Add unit test: delete non-existent key returns Ok(()) (idempotent)

**Checkpoint**: Delete operations persist correctly

---

## Phase 6: User Story 4 - Iterate Over All Entries (Priority: P2)

**Goal**: iterate_all returns snapshot-at-call-time view of all entries

**Independent Test**: put N entries + iterate_all returns exactly N entries; concurrent put during iterate doesn't affect result

### Implementation for User Story 4

- [ ] T036 [US4] Implement iterate_all() to acquire read lock and clone entire HashMap into Vec<(String, Vec<u8>)> in src/lib.rs (already done — verify works with RwLock)
- [ ] T037 [US4] Add unit test: put 100 entries + iterate_all returns exactly 100 with correct values
- [ ] T038 [US4] Add unit test: delete entry + iterate_all excludes it

**Checkpoint**: Iteration correctness validated

---

## Phase 7: User Story 5 - Thread-Safe Concurrent Access (Priority: P2)

**Goal**: Multiple threads performing concurrent operations without corruption or panics

**Independent Test**: Spawn 8+ threads doing random put/get/delete, verify no panics and data consistency

### Implementation for User Story 5

- [ ] T039 [US5] Verify RwLock implementation in src/lib.rs allows concurrent get() calls (read lock shared)
- [ ] T040 [US5] Implement checkpoint coalescing in src/flush.rs — multiple concurrent force_flush() calls share one flush operation (CheckpointCoalesce struct with Condvar)
- [ ] T041 [US5] Add stress test: 8 threads, 1000 operations each (random put/get/delete), assert no panics and final state consistent
- [ ] T042 [US5] Add stress test: concurrent iterate_all while other threads write, assert no panics and iteration returns consistent snapshot

**Checkpoint**: Thread safety validated under concurrent load

---

## Phase 8: User Story 6 - Force Flush to Disk (Priority: P3)

**Goal**: force_flush() provides immediate durability guarantee with coalescing

**Independent Test**: put + force_flush + crash-simulate (no background flush) + restart + get returns value

### Implementation for User Story 6

- [ ] T043 [US6] Implement background flush thread in src/flush.rs — Condvar::wait_timeout with configurable interval, wake on dirty_count threshold or explicit signal
- [ ] T044 [US6] Wire flush thread lifecycle in src/lib.rs — start thread after initialize(), stop thread on Drop (set shutdown flag + notify condvar)
- [ ] T045 [US6] Implement force_flush() in src/lib.rs — signal flush thread, block caller until flush completes (uses coalescing from T040)
- [ ] T046 [US6] Add FlushConfig struct with interval_secs and dirty_threshold fields, configurable via set_flush_config() method in src/lib.rs
- [ ] T047 [US6] Add unit test: put + force_flush + inject crash (no periodic flush) + reboot + verify entry persisted
- [ ] T048 [US6] Add unit test: verify dirty-count threshold triggers flush without waiting for timer
- [ ] T049 [US6] Add unit test: verify force_flush returns quickly when no dirty entries

**Checkpoint**: Explicit durability control and background flush both working

---

## Phase 9: Polish & Cross-Cutting Concerns

**Purpose**: Edge cases, capacity management, documentation

- [ ] T050 [P] Implement capacity checking in put() in src/lib.rs — calculate total serialized size, return CapacityExhausted if partition regions can't hold all entries
- [ ] T051 [P] Add unit test: fill store to capacity, verify CapacityExhausted error on next put, verify all existing entries intact
- [ ] T052 [P] Add unit test: put with zero-length value succeeds
- [ ] T053 [P] Add unit test: crash mid-flush (fault injection after partial region write) + reboot → recovers from previous valid region
- [ ] T054 Implement best-effort recovery logging in src/recovery.rs — log warning via ILogger for each skipped corrupt entry with key info
- [ ] T055 Run cargo clippy -- -D warnings on full crate and fix any issues
- [ ] T056 Run cargo doc --no-deps and ensure all public items have doc comments without warnings
- [ ] T057 Update Cargo.toml to gate src/block_io.rs, src/flush.rs, src/recovery.rs, src/on_disk.rs behind `#[cfg(feature = "spdk")]` compilation

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — can start immediately
- **Foundational (Phase 2)**: Depends on Phase 1 (on_disk types) — BLOCKS all user stories
- **User Story 1 (Phase 3)**: Depends on Phase 2 — core put/get with persistence
- **User Story 2 (Phase 4)**: Depends on Phase 3 (needs flush to test recovery)
- **User Story 3 (Phase 5)**: Depends on Phase 3 (needs basic put/get working)
- **User Story 4 (Phase 6)**: Depends on Phase 3 (needs entries in store)
- **User Story 5 (Phase 7)**: Depends on Phase 3 + Phase 4 (needs full lifecycle)
- **User Story 6 (Phase 8)**: Depends on Phase 3 + Phase 7 (needs coalescing from US5)
- **Polish (Phase 9)**: Depends on all user stories complete

### User Story Dependencies

- **US1 (P1)**: Foundational → immediately after Phase 2
- **US2 (P1)**: Depends on US1 (needs flush_to_disk working)
- **US3 (P2)**: Depends on US1 only (put/delete/flush)
- **US4 (P2)**: Depends on US1 only (put/iterate)
- **US5 (P2)**: Depends on US1 + US2 (full lifecycle for stress testing)
- **US6 (P3)**: Depends on US1 + US5 (coalescing from concurrency work)

### Parallel Opportunities

- T002, T003 can run in parallel with each other (different structs in same file)
- T008, T009, T010 can run in parallel (different functions in test_support.rs)
- T050, T051, T052, T053 can run in parallel (independent edge cases)
- US3 and US4 can run in parallel after US1 completes (independent stories)

---

## Parallel Example: Phase 2 (Foundational)

```bash
# These can be developed in parallel (different files):
Task: "Create src/test_support.rs with MockBlockDevice"          (T008)
Task: "Implement heap_dma_alloc() in src/test_support.rs"        (T009)
Task: "Implement FaultConfig in src/test_support.rs"             (T010)

# These must be sequential (same struct, dependencies):
Task: "Implement BlockDeviceClient struct"                        (T005)
Task: "Implement read_sectors"                                    (T006, depends on T005)
Task: "Implement write_sectors"                                   (T007, depends on T005)
```

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Complete Phase 1: Setup (on-disk format definitions)
2. Complete Phase 2: Foundational (I/O layer + MockBlockDevice)
3. Complete Phase 3: User Story 1 (put/get + flush to disk)
4. **STOP and VALIDATE**: write entries, flush, inspect mock disk state
5. This alone proves the on-disk format and I/O path work correctly

### Incremental Delivery

1. Setup + Foundational → on-disk format + I/O layer ready
2. Add US1 (put/get + flush) → core value path works (MVP!)
3. Add US2 (recovery) → store is truly persistent across restarts
4. Add US3 (delete) + US4 (iterate) in parallel → full CRUD
5. Add US5 (concurrency) → production-ready thread safety
6. Add US6 (background flush) → lazy persistence operational
7. Polish → capacity limits, crash recovery edge cases

---

## Notes

- All persistence code is gated behind `#[cfg(feature = "spdk")]` — without the feature, the component remains a pure in-memory store (current behavior preserved)
- MockBlockDevice tests validate the persistence logic without requiring SPDK hardware
- Integration test from spec 002 (tests/integration_ssd.rs) validates on real NVMe once this implementation is complete
- The existing 8 unit tests in src/lib.rs continue to pass throughout (in-memory mode)
