# Tasks

## Review Backfilled Spec

- [ ] Review generated user stories for accuracy
- [ ] Verify requirements match intended behavior
- [ ] Add any missing requirements (e.g., GPU registration flow)
- [ ] Confirm formal verification properties (P1-P10) are still current
- [ ] Mark spec status as "Draft" or "Approved"

## Validate Test Coverage

- [ ] Confirm all 12 unit tests in lib.rs pass (`cargo test -p memory-tier`)
- [ ] Confirm all 9 allocator tests pass
- [ ] Identify and document any untested error paths
- [ ] Verify Creusot verification conditions still discharge

## Address Test Gaps

- [ ] Add multi-threaded concurrency test (16 threads, mixed insert/get/evict)
- [ ] Add stress test for allocator fragmentation (random size allocation/deallocation)
- [ ] Add test for batch_touch with mixed present/absent keys
- [ ] Add test for oldest_keys ordering correctness
- [ ] Add test for evict_next_for_key targeting correct shard

## Add Benchmarks

- [ ] Create Criterion benchmark for single-threaded insert throughput
- [ ] Create Criterion benchmark for single-threaded get throughput (cache hit)
- [ ] Create Criterion benchmark for concurrent insert/get (16 threads)
- [ ] Create Criterion benchmark for eviction under full-pool steady state
- [ ] Add benchmark results to README.md

## Documentation

- [ ] Verify README.md matches current source layout (lru.rs removed, eviction delegated)
- [ ] Add architecture diagram showing shard layout and pointer arithmetic
- [ ] Document SPDK vs mmap allocation path differences
- [ ] Document NUMA binding behavior and fallback

## Future Enhancements (Backlog)

- [ ] Evaluate configurable shard count based on hardware thread count
- [ ] Design write-through pinning (NotEvictable usage)
- [ ] Add structured metrics (hit rate, eviction rate, fragmentation)
- [ ] Investigate buddy allocator for mixed-size workload support
- [ ] Implement CUDA host registration via pool_info() at the integration layer
