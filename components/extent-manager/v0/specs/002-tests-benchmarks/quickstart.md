# Quickstart: Tests and Benchmarks

## Running Tests

```bash
# Run all tests (existing + new)
cargo test -p extent-manager

# Run only API operation tests
cargo test -p extent-manager api_

# Run only power-failure tests
cargo test -p extent-manager crash_

# Run only thread-safety tests
cargo test -p extent-manager thread_

# Run benchmarks
cargo bench -p extent-manager

# CI gate (all must pass)
cargo fmt -p extent-manager --check \
  && cargo clippy -p extent-manager -- -D warnings \
  && cargo test -p extent-manager \
  && cargo doc -p extent-manager --no-deps \
  && cargo bench -p extent-manager --no-run
```

## Test Scenarios

### Scenario 1: Basic CRUD via IExtentManager

```text
1. Create MockBlockDevice with 10,000 blocks
2. Create ExtentManagerComponentV1, wire mock to block_device receptacle
3. Set flush_fn to no-op
4. Call initialize() with 2 size classes, 100 slots each
5. create_extent(key=1, class=0, filename="test.dat", crc=0, has_crc=false)
6. Verify extent_count() == 1
7. lookup_extent(key=1) → verify metadata matches
8. remove_extent(key=1) → verify extent_count() == 0
9. lookup_extent(key=1) → verify KeyNotFound error
```

### Scenario 2: Power-Failure Mid-Create

```text
1. Create MockBlockDevice, set fault_config to fail after 1 write
2. Wire and initialize ExtentManagerComponentV1
3. Attempt create_extent() — record write succeeds, bitmap write fails
4. Create NEW MockBlockDevice from same in-memory blocks (simulating reboot)
5. Wire to NEW component, call open()
6. Verify RecoveryResult.orphans_cleaned >= 1
7. Verify extent_count() == 0 (orphan was cleaned)
```

### Scenario 3: Concurrent Creates

```text
1. Create MockBlockDevice with large capacity
2. Wire and initialize ExtentManagerComponentV1 with Arc
3. Spawn 8 threads, each creating 100 extents with unique key ranges
4. Join all threads, verify no errors
5. Verify extent_count() == 800
6. Verify each extent is individually retrievable
```
