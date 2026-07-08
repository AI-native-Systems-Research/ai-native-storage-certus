# Tasks

## Review Backfilled Spec

- [ ] Review generated user stories for accuracy
- [ ] Verify requirements match intended behavior
- [ ] Add any missing requirements (e.g., key size limits, character restrictions)
- [ ] Confirm on-disk format version strategy for backward compatibility
- [ ] Mark spec status as "Draft" or "Approved"

## Documentation and Lint Compliance

- [ ] T056 Run `cargo doc --no-deps` and ensure all public items have doc comments without warnings
- [ ] T057 Consider whether `on_disk.rs` should remain always-compiled or be gated behind a feature flag

## Capacity Management Improvements

- [ ] Investigate pre-flight capacity check on `put()` to reject entries that would exceed on-disk region size before flush
- [ ] Document maximum entry count given partition size and average entry size
- [ ] Add `capacity_remaining()` or `estimated_usage()` method to the interface

## Recovery Robustness

- [ ] Add integration test for double-corruption scenario (both regions corrupt) on real hardware
- [ ] Add test for superblock corruption recovery (currently only region corruption is tested)
- [ ] Verify recovery behavior when partition size changes between reboots (e.g., disk replacement)

## Performance

- [ ] Add Criterion benchmark for put/get throughput (varied value sizes)
- [ ] Add Criterion benchmark for flush latency vs entry count
- [ ] Profile FlushManager coalescing behavior under high-concurrency workloads
- [ ] Evaluate whether `snapshot_entries()` clone can be replaced with a COW approach

## Feature Gating Cleanup

- [ ] Audit feature flag boundaries: ensure `on_disk.rs` public types are accessible without features for downstream consumers
- [ ] Verify that `testing` vs `spdk` feature separation is necessary or if they should be unified
- [ ] Document feature flag semantics in crate-level rustdoc

## Integration with Certus Server

- [ ] Document expected partition type GUID (`CERTUS_EXTERNAL_META`) and sizing guidelines
- [ ] Define how the dispatcher or server wires the `ILogger` receptacle at startup
- [ ] Specify expected initialization sequence in multi-component startup
