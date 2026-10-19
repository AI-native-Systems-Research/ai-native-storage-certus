# Tasks

## Review Backfilled Spec

- [ ] Review generated user stories for accuracy
- [ ] Verify requirements match intended behavior
- [ ] Add any missing requirements (e.g., key size limits, character restrictions)
- [ ] Confirm on-disk format version strategy for backward compatibility
- [ ] Mark spec status as "Draft" or "Approved"

## Documentation and Lint Compliance

- [ ] T056 Run `cargo doc --no-deps` and ensure all public items have doc comments without warnings — no longer blocked (ALIGN-001 resolved); undocumented public items are listed in `.specify/sync/drift-report.md` (SC-7) and proposal P5
- [x] T057 Consider whether `on_disk.rs` should remain always-compiled or be gated behind a feature flag — **Resolved** via spec-sync (2026-07-22): keep `on_disk.rs` always-compiled. It contains only on-disk format/data-structure definitions with no I/O dependency, distinct from the gated persistence I/O modules (`block_io`/`flush`/`recovery`/`test_support`). spec.md NFR-05 reworded accordingly; see `.specify/sync/apply-report.md`.

## Known Code Defects (tracked in `.specify/sync/align-tasks.md`)

- [x] ALIGN-001 (MAJOR, RESOLVED 2026-08-20): `components/extended-metadata-store` is now a member of the root `Cargo.toml` `[workspace] members` array and is built and tested by CI.
- [x] ALIGN-002 (MAJOR, RESOLVED 2026-08-20): `IExtendedMetadataStore::force_flush()` invokes the `FlushTrigger` installed via `attach_flush_trigger` (FR-05). Follow-up flush-failure propagation through `FlushManager::trigger_flush()` (ALIGN-EMS-003) — RESOLVED by `4a8fe3e7` (`src/flush.rs:181-185`; test `src/repair_test_force_flush_error.rs`).

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
