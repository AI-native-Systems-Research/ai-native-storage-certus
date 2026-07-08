# Tasks

## Review Backfilled Spec

- [ ] Review generated user stories for accuracy against actual code behavior
- [ ] Verify requirements FR-001 through FR-030 match implemented behavior
- [ ] Verify non-functional requirements NFR-001 through NFR-012 match implementation
- [ ] Remove implementation notes that don't belong in the spec (move to plan.md)
- [ ] Add any missing requirements discovered during code review
- [ ] Confirm key entity descriptions match current struct/type definitions
- [ ] Validate dependency table against actual Cargo.toml and receptacle wiring
- [ ] Mark spec status as "Draft" or "Approved"

## Review Backfilled Plan

- [ ] Confirm architecture diagram matches current code structure
- [ ] Verify data flow descriptions match actual code paths
- [ ] Validate design decision rationales are still current
- [ ] Check testing table matches actual test counts (may drift as tests are added)
- [ ] Review future considerations for relevance and priority

## Spec Accuracy: Populate Path

- [ ] Verify FR-002 (populate flow) matches `populate()` in lib.rs
- [ ] Confirm three-phase reserve/copy/complete is correctly described (FR-013 through FR-016)
- [ ] Verify eviction trigger behavior on pool-full matches FR-026 (alternating strategy)
- [ ] Confirm `AlreadyExists` error path via memory-tier `insert()` propagation (FR-004)
- [ ] Validate that `reserve_memory` checks for zero size independently of populate

## Spec Accuracy: Lookup Paths

- [ ] Verify hot-path lookup uses `warm_stream` AtomicU64 load (lock-free), not pipeline_ring lock
- [ ] Confirm `batch_lookup` classification loop matches FR-008 (hot inline, cold parallel)
- [ ] Verify remote lookup forwarding only fires for `KeyNotFound` entries (FR-009)
- [ ] Confirm `lookup_async` returns the CUDA stream used without blocking (FR-007)
- [ ] Validate dual-stream periodic sync interval (every 8 completions per NFR-005)
- [ ] Verify cold-path ColdReadPool fallback to scoped-thread inline execution (FR-027)

## Spec Accuracy: Background Writer

- [ ] Confirm ParallelBackgroundWriter routes jobs by `device_index % num_drives`
- [ ] Verify `flush_to_ssd()` blocks until in-flight count reaches zero (FR-019)
- [ ] Confirm `process_write_job` uses `peek()` (not `get()`) to avoid refreshing LRU
- [ ] Verify write-through uses MDTS-segmented I/O via `write_buffer_to_ssd` (NFR-004)
- [ ] Confirm extent publish + `convert_to_storage` only happens on successful write

## Spec Accuracy: Lifecycle

- [ ] Verify `initialize()` sequence matches FR-020 through FR-022 and NFR-011
- [ ] Confirm two-phase shutdown: signal-all, join-all, detach-all order (NFR-011)
- [ ] Verify extent manager checkpoint happens before block device teardown
- [ ] Confirm memory-tier pool unregistration from CUDA/SPDK during shutdown
- [ ] Verify cold pool shutdown happens before block device teardown (workers hold ClientChannels)
- [ ] Confirm re-initialization after shutdown works (destroy and recreate all state)

## Spec Accuracy: SSD Evictor

- [ ] Verify `BackgroundEvictor` monitors utilization at configurable intervals
- [ ] Confirm evictor only removes BlockDevice entries (skips MemoryTier entries)
- [ ] Verify evictor respects active references (remove fails gracefully)
- [ ] Confirm evictor stops when utilization drops below low_watermark

## Spec Accuracy: Drive Management

- [ ] Verify partition layout: metadata (128 MiB) + extended metadata (128 MiB) + data (remainder)
- [ ] Confirm extent manager configured with partition-relative LBA offsets
- [ ] Verify NUMA CPU assignment logic (round-robin per node or sequential from base)
- [ ] Confirm `create_data_drives` handles both factory and hard-coded SPDK paths

## Test Coverage Gaps

- [ ] Add test for `flush_to_ssd()` return value (count of flushed entries)
- [ ] Add test for `clear_memory_tier()` converting entries with ssd_offset to BlockDevice
- [ ] Add test for `touch()` refreshing both dispatch-map and memory-tier timestamps
- [ ] Add test for `batch_lookup` with mix of hot and cold entries
- [ ] Add test for `batch_lookup` remote lookup fallback path
- [ ] Add test for ColdReadPool submit and result collection
- [ ] Add test for pipeline MDTS segment splitting with real multi-segment transfers
- [ ] Add test for NUMA CPU assignment with mocked topology
- [ ] Add test verifying `release_memory` idempotency (absent key returns Ok)

## Documentation

- [ ] Ensure all public types have doc comments
- [ ] Verify `cargo doc --no-deps -p dispatcher` builds warning-free
- [ ] Update CLAUDE.md if architecture changes during review
- [ ] Add inline examples to key public methods where missing

## Formal Verification

- [ ] Create `components/dispatcher/verif/` directory for Spin/Promela models
- [ ] Define property P1: "A key never exists in both MemoryTier and BlockDevice simultaneously"
- [ ] Define property P2: "After shutdown completes, no background threads are running"
- [ ] Define property P3: "evict_for_space terminates within max_attempts iterations"
- [ ] Define property P4: "A populated key is always visible via check() after copy_gpu_to_memory_completed returns"
- [ ] Define property P5: "release_memory is idempotent and never panics"

## Performance Validation

- [ ] Run `cargo bench --bench dispatcher_benchmark` and record baseline
- [ ] Run `cargo bench --bench ssd_evictor_benchmark` and record baseline
- [ ] Profile hot-path lookup to verify it is lock-free (no mutex contention)
- [ ] Measure cold-path pipeline throughput vs raw NVMe bandwidth (target: >80%)
- [ ] Verify background writer does not become a bottleneck under high populate rate

## Integration Points

- [ ] Verify IDispatcher interface definition in `components/interfaces/` matches all methods implemented
- [ ] Confirm DispatcherConfig defaults are sensible for production workloads
- [ ] Validate that component builds correctly with `cargo build -p dispatcher` (spdk-backend feature)
- [ ] Validate that component builds without SPDK: `cargo build -p dispatcher --no-default-features`
- [ ] Run `cargo clippy -p dispatcher -- -D warnings` clean
- [ ] Run `cargo fmt -p dispatcher --check` clean
