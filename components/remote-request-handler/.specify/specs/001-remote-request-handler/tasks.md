# Tasks

## Review Backfilled Spec
- [ ] Review generated user stories for accuracy
- [ ] Verify requirements match intended behavior
- [ ] Remove implementation notes that don't belong in spec
- [ ] Add any missing requirements
- [ ] Mark spec status as "Draft" or "Approved"

## Implement IRemoteRequestHandler Trait Methods
- [ ] Wire `handle_lookup` to the resolve/release path (currently returns `NotInitialized`)
- [ ] Wire `handle_check` to query the dispatcher without RDMA Write
- [ ] Wire `handle_batch_lookup` to call the resolver for each key (non-RDMA path for local callers)
- [ ] Implement `release_lookup` to call the release callback
- [ ] Add unit tests for the wired trait methods using mock dispatcher

## Add Multi-Session Concurrency
- [ ] Spawn a dedicated thread per accepted connection in `serve_loop`
- [ ] Integrate `SessionRegistry` into the serve path to enforce max-session limits
- [ ] Add a session ID to log messages for disambiguation
- [ ] Handle CM disconnect events during active sessions (not just at accept time)
- [ ] Add integration test demonstrating two concurrent sessions

## Integrate TelemetryCollector into Serve Path
- [ ] Instantiate `TelemetryCollector` in `serve_loop` when `telemetry` feature is enabled
- [ ] Call `record_connection_accepted`/`rejected` on accept/reject
- [ ] Call `record_batch` after each `process_batch_with_rdma_write` completes
- [ ] Expose a `metrics()` method on the listener handle for external scraping
- [ ] Add test verifying counters increment correctly in the serve path

## Harden Error Handling in RDMA Path
- [ ] Replace `bail!` in `register_mr` with `RdmaError::AllocationFailed` for typed errors
- [ ] Add `errno` reporting to FFI failures (call `std::io::Error::last_os_error()`)
- [ ] Handle partial batch failure gracefully (some writes succeed, some fail)
- [ ] Add timeout configurability (currently hard-coded 10s poll timeout)
- [ ] Log QP state on persistent write failures for debuggability

## Add Connection Health Monitoring
- [ ] Detect `RDMA_CM_EVENT_DISCONNECTED` during active sessions
- [ ] Call `session.force_close()` on unexpected disconnect
- [ ] Clean up MRs and QP resources on abrupt disconnection
- [ ] Add a heartbeat mechanism or idle-session timeout
- [ ] Test forced disconnection cleanup with the mock RDMA layer

## Improve Build Robustness
- [ ] Add fallback for missing `protoc` on non-x86_64 platforms (currently only downloads linux-x86_64)
- [ ] Pin protoc version to a checksum for supply-chain safety
- [ ] Make RDMA library linking optional (allow build without libibverbs for CI/doc builds)
- [ ] Add `cfg(feature = "integration-test")` guards so hardware-dependent tests are not run by default
- [ ] Document build prerequisites in component README

## Expand Test Coverage
- [ ] Add property-based tests for protobuf encode/decode (fuzz with arbitrary bytes)
- [ ] Add session state machine exhaustive transition test (all valid/invalid pairs)
- [ ] Add benchmark for `process_batch_with_rdma_write` using MockRdmaOps (measure resolve+release overhead)
- [ ] Add test for `RdmaListener::shutdown()` unblocking behavior
- [ ] Add test for pool-MR vs per-entry-MR fallback path in batch processing

## Documentation
- [ ] Add module-level doc comments to `serve.rs` explaining the two-phase batch strategy
- [ ] Document the `Resolver` and `ReleaseCallback` lifetime requirements
- [ ] Add safety audit checklist for all `unsafe` blocks (verify each `// SAFETY:` comment)
- [ ] Document the protocol wire format for external implementors
- [ ] Update component README with architecture diagram and usage examples
