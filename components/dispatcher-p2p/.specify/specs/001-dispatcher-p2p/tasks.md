# Tasks

## Review Backfilled Spec

- [ ] Review generated user stories for accuracy against actual code behavior
- [ ] Verify requirements match intended behavior (not just current behavior)
- [ ] Remove implementation notes that don't belong in spec (e.g., internal constant values)
- [ ] Add any missing requirements (e.g., error handling for partial batch failures)
- [ ] Confirm non-functional requirements have measurable acceptance criteria
- [ ] Mark spec status as "Draft" or "Approved"

## Review Plan Architecture Documentation

- [ ] Verify ASCII component diagram matches actual receptacle bindings
- [ ] Confirm data flow descriptions match current code paths
- [ ] Validate key design decisions are still accurate post-cold-pool merge
- [ ] Check dependency table completeness against Cargo.toml

## Validate Test Coverage

- [ ] Audit unit test coverage for batch_lookup cold-path logic (currently untested without hardware)
- [ ] Verify promote_to_memory_tier parallel execution has test coverage
- [ ] Confirm cold_pool module has integration tests (currently no tests in cold_pool.rs)
- [ ] Add test for flush_to_ssd blocking behavior
- [ ] Add test for clear_memory_tier state transitions
- [ ] Add test for touch() updating LRU position
- [ ] Add test for release_memory idempotency
- [ ] Add test for remote_lookup forwarding in batch_lookup

## Verify Error Handling Completeness

- [ ] Audit all unwrap() calls in non-test code for panic safety
- [ ] Verify channel disconnection errors propagate correctly in background workers
- [ ] Confirm CUDA error codes are properly translated to DispatcherError
- [ ] Check that partial failures in batch_lookup do not corrupt shared state
- [ ] Verify extent_mgr.checkpoint() failure during shutdown does not prevent other cleanup

## Review Shutdown Correctness

- [ ] Verify three-phase shutdown ordering matches spec FR-015
- [ ] Confirm background workers drain all pending jobs before returning (FR-006)
- [ ] Test that shutdown after partial initialization does not panic
- [ ] Verify CUDA stream/buffer cleanup is complete (no leaks on repeated init/shutdown cycles)
- [ ] Confirm remote_lookup.leave_cluster() is called on shutdown

## Performance Baseline

- [ ] Run ssd_evictor_benchmark and record baseline numbers
- [ ] Profile hot-path lookup to verify zero-allocation claim (NFR-001)
- [ ] Measure warm_stream AtomicU64 load overhead vs mutex-guarded alternative
- [ ] Validate P2P ring initialization completes under 1 second (NFR-003)
- [ ] Confirm effective_qd of 16 per drive saturates PCIe in hardware benchmarks

## Documentation Sync

- [ ] Update component CLAUDE.md if plan.md path differs from referenced specs/001-gpudirect-cold-path/plan.md
- [ ] Ensure README.md (if exists) reflects current feature set
- [ ] Verify knowledge/ wiki entries reference correct module paths
- [ ] Document pipeline-telemetry feature flag usage in developer docs

## Code Quality

- [ ] Run cargo clippy -- -D warnings on dispatcher-p2p crate
- [ ] Verify cargo doc --no-deps produces no warnings
- [ ] Check for stale TODO/FIXME/NOTE comments that need resolution
- [ ] Review unsafe blocks for SAFETY comment completeness
- [ ] Audit noop_free + std::mem::forget patterns for soundness under panic unwind
- [ ] Verify all public APIs have doc comments with examples

## Concurrency Audit

- [ ] Review RwLock usage for potential deadlock (data_drives held during promote_and_serve)
- [ ] Verify AtomicBool ordering is correct for initialized flag (Acquire/Release pair)
- [ ] Check that cold_pool.lock().unwrap() inside batch_lookup cannot deadlock with shutdown
- [ ] Confirm backfill worker sleep does not hold any locks
- [ ] Audit dispatch_map reference counting for leak-free paths under all error conditions
