# Implementation Plan: Tests and Benchmarks

**Branch**: `002-tests-benchmarks` | **Date**: 2026-04-15 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `specs/002-tests-benchmarks/spec.md`

## Summary

Add comprehensive unit tests, simulated power-failure data integrity tests, thread-safety tests, and Criterion benchmarks for the extent-manager component. Requires a mock `IBlockDevice` implementation using in-memory storage and configurable fault injection. Tests run without SPDK hardware via heap-backed `DmaBuffer` allocation.

## Technical Context

**Language/Version**: Rust 1.75+ (edition 2021, stable toolchain)
**Primary Dependencies**: component-framework, component-core, component-macros, interfaces (with `spdk` feature)
**Storage**: In-memory mock block device (HashMap-based, 4KiB blocks)
**Testing**: `cargo test` (unit + integration + doc), Criterion 0.5 (benchmarks)
**Target Platform**: Linux (RHEL/Fedora)
**Project Type**: Library component (Rust crate)
**Performance Goals**: Benchmarks establish baselines — no specific throughput targets
**Constraints**: No SPDK/hugepages/NVMe required for tests. `DmaBuffer::from_raw()` for heap allocation.
**Scale/Scope**: ~100-200 new test cases, 4 benchmark groups

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Status | Notes |
|-----------|--------|-------|
| I. Correctness First | PASS | Tests cover all public APIs and error paths |
| II. Comprehensive Testing | PASS | Unit, integration, and doc tests for all public types. TDD approach used. |
| III. Performance Accountability | PASS | Criterion benchmarks for create/remove/lookup/count |
| IV. Documentation as Contract | PASS | New test_support module will have doc comments |
| V. Maintainability | PASS | Mock scoped to `#[cfg(test)]`, minimal public surface |
| VI. Component Framework Conformance | PASS | Tests use component receptacles and IExtentManager interface |

**Quality Gate compliance**: `cargo fmt --check` + `cargo clippy -D warnings` + `cargo test --all` + `cargo test --doc` + `cargo doc --no-deps` + `cargo bench --no-run`

## Project Structure

### Documentation (this feature)

```text
specs/002-tests-benchmarks/
├── plan.md              # This file
├── spec.md              # Feature specification
├── research.md          # Technical decisions
├── data-model.md        # Test entity definitions
├── quickstart.md        # Test execution guide
└── checklists/
    └── requirements.md  # Quality checklist
```

### Source Code

```text
src/
├── lib.rs               # MODIFY — add #[cfg(test)] mod tests with API tests
├── block_device.rs      # MODIFY — add DMA allocator injection for tests
├── bitmap.rs            # existing (12 tests retained)
├── metadata.rs          # existing (13 tests retained)
├── superblock.rs        # existing (10 tests retained)
├── error.rs             # existing (9 tests retained)
├── recovery.rs          # MODIFY — add #[cfg(test)] mod tests for recovery
└── test_support.rs      # NEW — MockBlockDevice, HeapDmaBuffer, FaultConfig

tests/
├── api_operations.rs    # NEW — IExtentManager full-path integration tests
├── crash_recovery.rs    # NEW — simulated power-failure scenarios
└── thread_safety.rs     # NEW — concurrent operation tests

benches/
├── create_benchmark.rs  # NEW — create_extent throughput
├── lookup_benchmark.rs  # NEW — lookup_extent throughput
├── remove_benchmark.rs  # NEW — remove_extent throughput
└── count_benchmark.rs   # NEW — extent_count throughput
```

**Structure Decision**: Tests are split across inline `#[cfg(test)]` modules (unit tests for internal logic) and the `tests/` directory (integration tests exercising the full component through `IExtentManager`). `test_support.rs` is a `#[cfg(test)]` module providing shared mock infrastructure. Benchmarks in `benches/` follow the Criterion convention.

## Implementation Strategy

### Phase 1: Test Infrastructure (MockBlockDevice)

1. **`src/test_support.rs`** — Mock block device and heap DMA utilities:
   - `heap_dma_alloc(size, align, numa_node)` → `DmaBuffer` via `from_raw()` with heap memory
   - `MockBlockDevice` implementing all 10 `IBlockDevice` methods
   - Actor thread: processes `ReadSync`/`WriteSync` commands against `HashMap<u64, [u8; 4096]>`
   - `FaultConfig` struct with `fail_after_n_writes`, `fail_lba_range`, `fail_all_writes`
   - Helper: `create_test_component(num_blocks, sizes, slots)` → wired `ExtentManagerComponentV1`

2. **`src/block_device.rs`** — DMA allocator injection:
   - Add `DmaAllocFn` type alias: `Arc<dyn Fn(usize, usize, Option<i32>) -> Result<DmaBuffer, String> + Send + Sync>`
   - Add `dma_alloc` field to `BlockDevice`
   - `BlockDevice::new()` defaults to `DmaBuffer::new` wrapper
   - `BlockDevice::new_with_alloc()` accepts custom allocator (used in tests)
   - `read_block()` and `write_block()` call `(self.dma_alloc)(...)` instead of `DmaBuffer::new()`

### Phase 2: API Operation Tests

3. **`tests/api_operations.rs`** — Integration tests via `IExtentManager`:
   - `test_create_and_lookup`: create extent, verify count, lookup metadata
   - `test_create_and_remove`: create then remove, verify count back to 0
   - `test_duplicate_key_error`: create same key twice
   - `test_key_not_found_error`: lookup/remove non-existent key
   - `test_invalid_size_class_error`: create with out-of-range size class
   - `test_out_of_space_error`: fill all slots, verify error
   - `test_not_initialized_error`: call create before initialize
   - `test_device_too_small_error`: initialize on small device
   - `test_multiple_size_classes`: create extents across all classes
   - `test_extent_metadata_round_trip`: verify to_bytes/from_bytes through interface
   - `test_filename_and_crc`: create with filename and CRC, verify in lookup
   - `test_initialize_and_reopen`: initialize, populate, re-open, verify recovery

4. **`src/lib.rs`** — Unit tests for component internals:
   - `test_set_flush_fn`: verify flush_fn storage
   - `test_get_state_not_initialized`: verify error before init

### Phase 3: Power-Failure Simulation Tests

5. **`tests/crash_recovery.rs`** — Simulated crash scenarios:
   - `test_orphan_after_record_write`: fail bitmap write after record write, verify orphan detection
   - `test_consistency_after_bitmap_fail_on_remove`: fail bitmap clear on remove, verify extent survives
   - `test_recovery_after_clean_shutdown`: normal init/populate, re-open, verify all extents
   - `test_recovery_statistics`: verify RecoveryResult fields match expectations
   - `test_corrupt_superblock_on_open`: write garbage to block 0, verify error

### Phase 4: Thread-Safety Tests

6. **`tests/thread_safety.rs`** — Concurrent operation tests:
   - `test_concurrent_creates`: N threads each creating unique extents, verify total count
   - `test_concurrent_creates_and_removes`: mixed create/remove, verify consistency
   - `test_concurrent_lookups`: many threads reading same keys, verify no deadlock
   - `test_concurrent_mixed_operations`: creates + removes + lookups simultaneously
   - All tests use `Arc<ExtentManagerComponentV1>` shared across threads
   - Deadlock detection via `thread::Builder::new().spawn()` with 30-second test timeout

### Phase 5: Benchmarks

7. **Benchmark files** (`benches/`):
   - `create_benchmark.rs`: Criterion group benchmarking `create_extent` throughput
   - `lookup_benchmark.rs`: Criterion group benchmarking `lookup_extent` throughput
   - `remove_benchmark.rs`: Criterion group benchmarking `remove_extent` throughput (create then remove)
   - `count_benchmark.rs`: Criterion group benchmarking `extent_count` (with varying extent counts)

8. **`Cargo.toml`** — Uncomment `[[bench]]` entries and add `test_support` as `path` (cfg-test module doesn't need explicit entry).

### Phase 6: Polish

9. **Update `Cargo.toml`**: Uncomment bench entries, verify dev-dependencies
10. **Run full CI gate**: `cargo fmt --check && cargo clippy -D warnings && cargo test --all && cargo doc --no-deps && cargo bench --no-run`
11. **Fix any issues** from CI gate

## Key Technical Decisions (from research.md)

| Decision | Choice | Why |
|----------|--------|-----|
| DMA in tests | `DmaBuffer::from_raw()` with heap | Avoids SPDK runtime dependency |
| Mock architecture | `IBlockDevice` impl with SpscChannel + HashMap | Tests full component path |
| DMA injection | Allocator closure in `BlockDevice` | Decouples allocation from I/O |
| Fault injection | `Arc<Mutex<FaultConfig>>` shared state | Thread-safe, configurable per-write |
| Thread tests | `std::thread` with `Arc` | Exercises real sync primitives |
| Benchmarks | Criterion 0.5 with mock | Deterministic, CI-compatible |
