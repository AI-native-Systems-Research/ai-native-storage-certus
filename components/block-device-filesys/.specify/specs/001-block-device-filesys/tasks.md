# Tasks

## Review Backfilled Spec
- [ ] Review generated user stories for accuracy
- [ ] Verify requirements match intended behavior
- [ ] Remove implementation notes that don't belong in spec
- [ ] Add any missing requirements
- [ ] Mark spec status as "Draft" or "Approved"

## Telemetry Accuracy
- [ ] Fix `start_ns` recording in `InflightOp` — currently uses `Instant::now().elapsed().as_nanos()` which is always ~0 (elapsed since just-created instant); should capture `Instant::now()` and compute latency at completion time
- [ ] Add per-operation latency recording for sync path (currently passes 0 as latency_ns to `record_op`)
- [ ] Add separate read vs write op counters in `TelemetryStats`
- [ ] Add integration test for telemetry feature gate (`cargo test --features telemetry`)

## Error Handling Improvements
- [ ] Return a more specific error than `NvmeBlockError::NotInitialized` when the io_uring submission queue is full (consider a dedicated `QueueFull` variant or retry logic)
- [ ] Handle partial pread/pwrite returns (currently treats any non-negative return as success, even if fewer bytes were transferred than requested)
- [ ] Add logging for LBA validation failures at debug level

## Actor Robustness
- [ ] Remove dead client sessions when `callback_tx.send()` fails (currently logs a warning but leaves the session in the map)
- [ ] Add graceful draining of inflight ops on shutdown (currently the actor exits immediately without waiting for io_uring completions)
- [ ] Consider bounding the number of concurrent clients to prevent unbounded HashMap growth

## io_uring Path Enhancements
- [ ] Batch multiple SQE submissions per `on_idle()` cycle before calling `ring.submit()` (reduce syscall overhead)
- [ ] Use io_uring timeout SQEs for precise deadline enforcement instead of polling in `check_timeouts()`
- [ ] Handle io_uring CQE overflow (ring overflow flag)

## Benchmark Coverage
- [ ] Add async IO latency benchmark (currently only measures sync path)
- [ ] Add multi-client concurrent throughput benchmark
- [ ] Add benchmark for write-zeros throughput vs manual zero-buffer writes
- [ ] Document expected latency ranges in benchmark README

## Documentation
- [ ] Add module-level doc comments to `actor.rs` explaining the io_uring linked-fsync pattern
- [ ] Document the bit-63 fsync CQE identification convention
- [ ] Add architecture diagram to component README
- [ ] Document the O_DIRECT fallback behavior and its implications for benchmarking

## Code Quality
- [ ] Remove unused `crossbeam-channel` dependency from Cargo.toml
- [ ] Audit all `unsafe` blocks for completeness of SAFETY comments (some lack detail on lifetime guarantees)
- [ ] Consider replacing raw `libc::posix_memalign` + `libc::free` in `handle_write_zeros` with a safe aligned-allocation wrapper
- [ ] Replace `HashMap<u64, ClientSession>` key collection + re-lookup pattern in `poll_clients()` with a more efficient iteration approach
