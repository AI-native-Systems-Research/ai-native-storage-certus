# Tasks

## Review Backfilled Spec
- [ ] Review generated user stories for accuracy
- [ ] Verify requirements match intended behavior
- [ ] Remove implementation notes that don't belong in spec
- [ ] Add any missing requirements
- [ ] Mark spec status as "Draft" or "Approved"

## Fix Telemetry Latency Recording
- [ ] In `actor.rs` `handle_read_sync()` and `handle_write_sync()`, record actual per-operation latency (capture `Instant::now()` before SQE submission, compute elapsed after CQE)
- [ ] In `handle_read_async()` and `handle_write_async()`, store meaningful `start_ns` in `InflightOp` (currently uses `Instant::now().elapsed().as_nanos()` which is always ~0)
- [ ] In `harvest_completions()`, compute elapsed time from `InflightOp.start_ns` and pass to `telemetry.record_op()`
- [ ] Add unit test verifying `TelemetryStats::snapshot()` reports non-zero latency after `record_op()` with real values

## Architecture Portability
- [ ] Replace hardcoded `BLKGETSIZE64` ioctl constant `0x80081272` with a cross-architecture definition (conditional compilation for x86_64 vs aarch64)
- [ ] Consider using `nix` crate's ioctl macros for safer cross-platform ioctl calls
- [ ] Add compile-time assertion or `cfg` gate that prevents silent miscompilation on unsupported architectures

## Improve Actor CPU Affinity
- [ ] Implement `set_actor_cpu()` using `libc::sched_setaffinity` to pin actor thread to specified core
- [ ] Store configured CPU in component fields and apply on `activate()`
- [ ] Add validation that the CPU index is valid (within `nproc` range)
- [ ] Document NUMA-aware placement recommendation in component README

## Graceful Client Disconnect Handling
- [ ] Detect broken callback channel in `send_completion()` and automatically remove the client from `clients` map
- [ ] Add `ControlMessage::DisconnectClient` usage path from the component side (currently only actor-initiated)
- [ ] Clean up inflight ops belonging to a disconnected client (cancel io_uring ops, remove from inflight map)
- [ ] Add integration test for client-drop-while-ops-inflight scenario

## True io_uring Batch Submission
- [ ] Refactor `BatchSubmit` handling to queue multiple SQEs before a single `ring.submit()` call
- [ ] Track batch operations as a group for completion correlation
- [ ] Benchmark batch submission vs current sequential approach at 8/32/128 ops per batch
- [ ] Ensure partial batch failures are reported per-operation (not all-or-nothing)

## Expand Test Coverage Without Hardware
- [ ] Create a loopback block device test fixture (`losetup`) for CI integration tests
- [ ] Add test for `BatchSubmit` with mixed read/write operations
- [ ] Add test for async operation timeout path (requires controllable slow device or mock)
- [ ] Add test for `AbortOp` verifying inflight map cleanup
- [ ] Add stress test for rapid connect/disconnect of multiple clients

## Documentation
- [ ] Add rustdoc examples to all public methods in `lib.rs` (currently only `create()` has one)
- [ ] Document the `telemetry` feature flag behavior in crate-level docs
- [ ] Add `BENCH_DEVICE_PATH` / `TEST_BLOCK_DEVICE` env var documentation to README
- [ ] Ensure `cargo doc --no-deps -p block-device-kernel` is warning-free
