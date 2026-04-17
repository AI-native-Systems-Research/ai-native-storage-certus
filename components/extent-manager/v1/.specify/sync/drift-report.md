# Spec Drift Report

Generated: 2026-04-17
Project: extent-manager v1

## Summary

| Category | Count |
|----------|-------|
| Specs Analyzed | 2 |
| Requirements Checked | 31 (16 + 15) |
| Aligned | 21 (68%) |
| Drifted | 5 (16%) |
| Not Implemented | 5 (16%) |
| Unspecced Code | 1 |

## Detailed Findings

### Spec: 001-extent-management - Extent Management

#### Aligned

- FR-001: Create extents with key, size, filename, CRC. `IExtentManager::create_extent(key, extent_size, filename, data_crc)` at `src/lib.rs:150`. Tests: `create_extent_basic`, `create_extent_with_filename_and_crc`.
- FR-002: Allocate contiguous disk space from size class. Slab-based allocation with `find_free_slot(extent_size)` in `src/state.rs`. Tests: `dynamic_slab_allocation`, `multi_slab_same_class`.
- FR-004: Remove by key, free space. `remove_extent` at `src/lib.rs:226`. Tests: `remove_extent_basic`, `remove_then_create_reuses_slot`.
- FR-005: Lookup by key. `lookup_extent` at `src/lib.rs:266`. Tests: `lookup_extent_basic`, `lookup_extent_not_found`.
- FR-006: Iteration through all stored extents. `get_extents()` at `src/lib.rs:277` returns `Vec<Extent>`. Each extent visited exactly once. Read lock blocks concurrent writers. Tests: `get_extents_empty`, `get_extents_returns_all`, `get_extents_reflects_removals`.
- FR-010: Thread-safe access. `RwLock<ExtentManagerState>` at `src/lib.rs:31`; write lock for create/remove, read lock for lookup/get_extents. Tests: `concurrent_creates_no_duplicates`, `concurrent_lookups`, `concurrent_create_and_lookup`.
- FR-011: Error types. `ExtentManagerError` enum: `DuplicateKey`, `KeyNotFound`, `OutOfSpace`, `NotInitialized`, `IoError`, `CorruptMetadata`. Tests: `create_extent_duplicate_key_fails`, `lookup_extent_not_found`, `not_initialized_errors`, `out_of_space`.
- FR-013: Block device receptacle. `block_device: IBlockDevice` in `define_component!` at `src/lib.rs:27`.
- FR-014: Logger receptacle. `logger: ILogger` in `define_component!` at `src/lib.rs:28`. Used via `log_info()` and `log_debug()` at `src/lib.rs:43-53`.
- FR-016: Error propagation. All I/O errors mapped via `error::nvme_to_em` and returned immediately, no retries.

#### Drifted

- FR-003: Spec says "persist extent metadata (key, namespace ID, offset, size, optional filename, optional CRC)." Code no longer stores namespace ID — intentionally removed from `Extent` and `ExtentMetadata`. Clients create one ExtentManager per namespace/device pair.
  - Location: `src/metadata.rs`, `interfaces/src/iextent_manager.rs`
  - Severity: **major** (intentional API redesign, spec needs update)

- FR-006: Spec says "iteration MUST hold an exclusive lock." `get_extents()` uses `state.read()` (shared lock). The RwLock blocks writers while readers hold the lock, satisfying the key guarantee, but multiple iterations can run concurrently.
  - Location: `src/lib.rs:277-280`
  - Severity: **minor** (functional guarantee met; spec language overly strict)

- FR-007: Spec says "valid size classes range from 128 KiB to 5 MiB and must be multiples of 4 KiB." Code has no validation of `extent_size` in `create_extent` — any u32 value is accepted. Slab validation (>= 8KiB, multiple of 4KiB) is correct.
  - Location: `src/lib.rs:150-156`
  - Severity: **moderate** (missing input validation)

- FR-012: Spec says "System MUST support fresh initialization AND reopening an existing volume." `open()` was intentionally removed — no reopen/recovery capability.
  - Location: `src/lib.rs`
  - Severity: **major** (intentional removal, spec needs update)

- Key Entities: Spec describes Superblock at block 0. `superblock.rs` was deleted. Initialization is purely in-memory. No on-disk superblock.
  - Severity: **major** (intentional removal, spec needs update)

#### Not Implemented

- FR-008: Crash consistency via atomic 4KiB writes. `recovery.rs` was deleted. No crash recovery mechanism exists.
- FR-009: Detect and recover from partial writes on startup. `open()` was removed.
- FR-012 (reopen): Reopening an existing volume is not supported.
- FR-015: Iteration performance for rebuilding indexes at startup via `open()`. `open()` was removed.
- User Story 5: Crash Recovery (all 4 acceptance scenarios). Intentionally removed.

---

### Spec: 002-extent-benchmark - Extent Manager Benchmark Application

#### Aligned

- FR-001: Standalone binary at `apps/extent-benchmark/`. Exists.
- FR-002: `--device <PCI_ADDRESS>`. Present in `config.rs:9`.
- FR-003: `--ns-id <NAMESPACE_ID>` (default 1). Present in `config.rs:12`. Used for block device capacity queries only (extent manager no longer takes ns_id).
- FR-004: `--threads <N>` (default 1). Present in `config.rs:15`.
- FR-005: `--count <N>` (default 10,000). Present in `config.rs:18`.
- FR-006: `--size-class <BYTES>` (default 131072). Present in `config.rs:21`.
- FR-007: `--slab-size <BYTES>` (default 1 GiB). Present in `config.rs:28`.
- FR-008: `--total-size <BYTES>` (auto-detect). `Option<u64>` in `config.rs:35`.
- FR-010: Three phases in order. `main.rs` runs create, lookup, remove sequentially.
- FR-011: Disjoint key ranges. `compute_key_ranges()` in `main.rs`.
- FR-014: README.md exists at `apps/extent-benchmark/README.md`.
- FR-015: Partial results on errors. `run_create/run_lookup/run_remove` log errors per-op and continue.

#### Drifted

- FR-003: `--ns-id` is used for block device capacity queries (`ibd.num_sectors(config.ns_id)`) but not passed to `iem.initialize()` which no longer takes namespace_id.
  - Location: `apps/extent-benchmark/src/main.rs:107-118`
  - Severity: **minor** (functional but semantics shifted)

- FR-009: Spec says "wire full component stack including Logger." Logger binding to extent manager not visible in main.rs.
  - Severity: **minor**

#### Not Implemented

(None — all FR requirements are implemented)

---

### Unspecced Code

| Feature | Location | Suggested Spec |
|---------|----------|----------------|
| `set_dma_alloc` in IExtentManager trait | `iextent_manager.rs:46` | 001 (add FR for DMA configuration) |

## Inter-Spec Conflicts

- Spec 001 FR-003 references "namespace ID" in extent metadata, but namespace_id was intentionally removed from the API.
- Spec 001 FR-012 requires `open()` for reopening volumes, but this was intentionally removed along with crash recovery (FR-008, FR-009).
- Spec 002 FR-003 references `--ns-id` for extent manager initialization, but `initialize()` no longer takes namespace_id.

## Recommendations

1. **Update spec 001** to reflect intentional API changes: removal of namespace_id/device_id, removal of open()/recovery/superblock, removal of extent_count, addition of get_extents(), set_dma_alloc.
2. **Add extent_size validation** in `create_extent` if the 128KiB-5MiB range constraint is still desired, or update spec to remove this constraint.
3. **Update spec 001 FR-006** to describe actual locking: `get_extents()` holds read lock (writers blocked, concurrent reads allowed).
4. **Decide on crash recovery**: re-spec as future feature or remove from spec entirely.
5. **Update spec 002** to reflect that `--ns-id` is used only for block device capacity queries.
