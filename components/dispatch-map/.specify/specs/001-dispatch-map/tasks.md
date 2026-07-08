# Tasks

## Review Backfilled Spec
- [ ] Review generated user stories for accuracy against actual implementation behavior
- [ ] Verify requirements FR-001 through FR-019 match implemented methods in `src/lib.rs`
- [ ] Verify NFR-001 through NFR-009 match implementation constraints
- [ ] Remove implementation notes that don't belong in spec (move to plan.md if needed)
- [ ] Add any missing requirements (e.g., `entry_size` calculation, `touch` LRU semantics)
- [ ] Mark spec status as "Draft" or "Approved"

## Concurrency Model Improvements
- [ ] Evaluate replacing global `Mutex<Inner>` + `Condvar` with sharded map (e.g., 16 shards by key hash) to reduce lock contention under high thread counts
- [ ] Profile `notify_all()` overhead — assess whether per-key wait structures (futex or `WaitGroup`) would reduce spurious wakeups
- [ ] Add contention metrics: track timeout count, average wait duration, and peak concurrent readers per key
- [ ] Benchmark multi-threaded lookup throughput with 8+ threads to establish contention baseline

## Timeout Configurability
- [ ] Promote `DEFAULT_TIMEOUT` from compile-time `const` to a runtime-configurable field on `DispatchMapState`
- [ ] Allow per-operation timeout overrides (add `lookup_with_timeout`, `take_read_with_timeout` variants, or accept `Option<Duration>`)
- [ ] Document timeout semantics in interface doc comments

## Capacity Management
- [ ] Add `len()` method to `IDispatchMap` to report current entry count
- [ ] Add optional capacity limit with back-pressure signal (return `DispatchMapError::CapacityExceeded` when full)
- [ ] Integrate automatic eviction trigger when entry count exceeds a high-water mark

## Batch Operations
- [ ] Add `lookup_batch(keys: &[CacheKey])` to amortize single mutex acquisition over multiple keys
- [ ] Add `release_read_batch(keys: &[CacheKey])` for bulk release in scatter-gather I/O paths
- [ ] Benchmark batch vs. individual operations at various batch sizes

## Entry Size Optimization
- [ ] Audit `DispatchEntry` layout — current size validated at <= 56 bytes via benchmark assertion
- [ ] Evaluate packing `read_ref` and `write_ref` into a single `AtomicU64` (32 bits each) to enable lock-free fast-path reads
- [ ] Consider replacing `EvictionHandle` (8 bytes) with a generation-indexed slot for tighter packing

## Test Coverage Gaps
- [ ] Add property-based tests (e.g., `proptest`) for reference counting invariants: `read_ref` never goes negative, `write_ref` always 0 or 1
- [ ] Add stress test: 100+ threads performing random operations on 100 keys for 5 seconds, verifying no panics or state corruption
- [ ] Add test for `RefCountOverflow` scenario (read_ref approaching `u32::MAX`)
- [ ] Add test for `is_evictable` predicate across all state combinations
- [ ] Add test for `recover_extent` duplicate key rejection
- [ ] Add test for `entry_size` calculation correctness

## Documentation
- [ ] Add module-level doc comments to `entry.rs` and `state.rs` explaining their role
- [ ] Document the two-phase write-through lifecycle in a doc comment on `convert_to_storage`
- [ ] Add usage examples to `IDispatchMap` trait methods showing typical lookup-use-release flow
- [ ] Ensure `cargo doc -p dispatch-map --no-deps` produces zero warnings

## Formal Verification Maintenance
- [ ] Verify Creusot proofs still discharge after any code changes (P1-P10)
- [ ] Add Spin/Promela model for condvar wait/notify protocol correctness (blocking semantics are currently unchecked)
- [ ] Model the two-phase state machine (MemoryTier -> ssd_offset set -> BlockDevice) as a verified state transition

## Recovery Robustness
- [ ] Add integration test for recovery with duplicate keys in extent manager (should not panic)
- [ ] Add integration test for recovery when eviction policy returns error from `track()`
- [ ] Consider adding a `clear()` method to reset the map (useful for re-initialization scenarios)

## Observability
- [ ] Add structured logging with log levels (currently uses `info` for recovery, `debug` for operations)
- [ ] Emit metrics for: total lookups, cache hits by tier (memory vs block), timeouts, evictions
- [ ] Add `stats()` method to return a snapshot of operational counters
