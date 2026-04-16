# Research: Tests and Benchmarks

## Decision 1: DmaBuffer Allocation in Tests

**Decision**: Use `DmaBuffer::from_raw()` with heap-allocated memory in test builds to avoid SPDK runtime dependency.

**Rationale**: `DmaBuffer::new()` calls `spdk_sys::spdk_dma_zmalloc`, which requires a running SPDK environment. `DmaBuffer::from_raw()` (interfaces/src/spdk_types.rs:276–299) accepts any raw pointer and a C-ABI deallocator — it does not call any SPDK function. By using `std::alloc::alloc_zeroed` with a 4096-byte aligned layout, we can construct DmaBuffers from ordinary heap memory for tests.

**Alternatives considered**:
- Adding a `test-alloc` feature to the `interfaces` crate: Too invasive, requires modifying a shared crate for one component's tests.
- Not going through `BlockDevice` at all: Would bypass the actual I/O path being tested.
- Using `cfg(test)` on `DmaBuffer::new()`: Requires modifying the interfaces crate.

## Decision 2: Mock IBlockDevice Architecture

**Decision**: Create a `MockBlockDevice` struct that implements `IBlockDevice` using `SpscChannel` from `component_core::channel` for command/completion transport and a `HashMap<u64, [u8; 4096]>` for in-memory block storage.

**Rationale**: `IBlockDevice` requires 10 methods. The mock only needs functional implementations of `connect_client()`, `sector_size()`, `num_sectors()`, and `numa_node()` — the rest can return safe defaults. The mock's actor thread processes `Command::ReadSync`/`WriteSync` and sends `Completion::ReadDone`/`WriteDone`. `SpscChannel` is pure Rust (no SPDK dependency).

**Alternatives considered**:
- Trait-based `BlockIO` abstraction: Was previously removed; re-introducing it adds indirection to production code for test convenience.
- Testing at `ExtentManagerState` level only: Would not test the `IExtentManager` → `BlockDevice` → channel path.

## Decision 3: DMA Allocation Abstraction in BlockDevice

**Decision**: Add a `dma_alloc` closure field to `BlockDevice` (defaulting to `DmaBuffer::new` in production, overridable to heap allocation in tests) to decouple DMA allocation from the I/O path.

**Rationale**: `BlockDevice::read_block()` and `write_block()` hardcode `DmaBuffer::new(...)`. The mock IBlockDevice provides channels, but BlockDevice still calls SPDK-linked DMA allocation. Injecting the allocator via a closure allows tests to use heap DmaBuffers while production code uses SPDK DMA.

**Alternatives considered**:
- `cfg(test)` conditional in `BlockDevice`: Works but is fragile — can't test with real SPDK and mock SPDK in the same binary.
- Separate `BlockDeviceForTest` struct: Code duplication.

## Decision 4: Fault Injection Strategy

**Decision**: The mock's actor thread checks an `Arc<Mutex<FaultConfig>>` before processing each command. `FaultConfig` supports: fail after N writes, fail specific LBA ranges, and fail all writes.

**Rationale**: Power-failure simulation requires controlled failure injection at the block I/O level. The two-phase write protocol (record write → bitmap persist) means we need to fail the second write but allow the first to succeed. An `Arc<Mutex<>>` config is thread-safe and can be modified between operations.

**Alternatives considered**:
- Callback-based injection (closure per operation): More flexible but harder to reason about.
- AtomicBool kill switch: Too coarse — can't selectively fail specific writes.

## Decision 5: Thread-Safety Test Approach

**Decision**: Use `std::thread::spawn` with `Arc<ExtentManagerComponentV1>` sharing. Each thread gets unique key ranges. Verify final state via `extent_count()` and individual lookups.

**Rationale**: The component uses interior mutability (RwLock, Mutex). Testing with standard threads (not async) exercises the actual synchronization path. Unique key ranges per thread avoid testing HashMap collision behavior and focus on lock correctness.

**Alternatives considered**:
- Using `rayon` for parallel iteration: Adds a dependency for tests only.
- Async tasks with tokio: Component is sync-only, async would test wrong code path.

## Decision 6: Benchmark Infrastructure

**Decision**: Use Criterion 0.5 (already in dev-dependencies). Benchmarks use the mock block device. Four benchmark groups: create, remove, lookup, extent_count.

**Rationale**: Constitution mandates Criterion. Mock block device provides deterministic, reproducible benchmarks measuring algorithmic overhead (not I/O latency). Benchmark groups align with the IExtentManager interface.

**Alternatives considered**:
- Benchmarking with real SPDK: Not available in CI.
- Using `std::hint::black_box` only: Doesn't provide statistical analysis.
