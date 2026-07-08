# Tasks: Block Device Kernel Component

**Input**: Backfilled from existing implementation at `components/block-device-kernel/`

**Prerequisites**: Component is fully implemented. These tasks focus on spec review and potential improvements.

**Organization**: Tasks are grouped by review area and potential enhancement.

## Review Backfilled Spec

- [ ] Review generated user stories for accuracy against actual usage patterns
- [ ] Verify requirements match intended behavior (not just observed behavior)
- [ ] Add any missing requirements (e.g., NUMA pinning intent, future multi-queue design)
- [ ] Mark spec status as "Draft" or "Approved"
- [ ] Confirm edge case list is exhaustive for production deployment

## Spec Accuracy Verification

- [ ] T001 Verify FR-005 (io_uring only, no fallback) matches design intent — confirm this is deliberate, not a missing feature
- [ ] T002 Verify FR-004 (O_DSYNC for durability) is sufficient for all target workloads — no scenarios requiring explicit fsync
- [ ] T003 Confirm single-namespace model (ns_id=1) is intentional and not a placeholder for future multi-namespace support
- [ ] T004 Verify CLIENT_CHANNEL_CAPACITY=64 is adequate for production workloads or document rationale
- [ ] T005 Verify DEFAULT_RING_DEPTH=128 is optimal for target NVMe devices or document tuning guidance

## Testing Coverage Gaps

- [ ] T006 Add unit test for `DeviceConfig` overflow case (block_size * num_blocks > u64::MAX)
- [ ] T007 Add integration test for `BatchSubmit` with multiple mixed read/write operations
- [ ] T008 Add integration test for `AbortOp` on an in-flight async operation
- [ ] T009 Add integration test for async operation timeout (`Completion::Timeout`)
- [ ] T010 Add integration test verifying telemetry accuracy with `--features telemetry`
- [ ] T011 Add integration test for shutdown while async operations are in-flight
- [ ] T012 Add integration test for rapid connect/disconnect of multiple clients

## Documentation Gaps

- [ ] T013 Add doc examples for `BlockDeviceKernelComponent::initialize()` and `shutdown()`
- [ ] T014 Add doc examples for `DeviceConfig` construction with auto-detect
- [ ] T015 Document minimum kernel version requirement in crate-level docs
- [ ] T016 Document required system permissions (root/disk group) for block device access
- [ ] T017 Add README.md with quickstart guide for the component

## Potential Improvements (Future Work)

- [ ] T018 Evaluate IORING_SETUP_SQPOLL for reduced submission latency
- [ ] T019 Evaluate IORING_REGISTER_BUFFERS for pre-registered DMA buffers
- [ ] T020 Implement NUMA node detection from block device sysfs path
- [ ] T021 Consider per-CPU io_uring rings for multi-queue scaling
- [ ] T022 Add configurable ring depth (currently hardcoded to 128)
- [ ] T023 Add configurable channel capacity (currently hardcoded to 64)
- [ ] T024 Evaluate io_uring probe at init to adapt to kernel capabilities
- [ ] T025 Consider adding per-client telemetry breakdown

## Code Quality

- [ ] T026 Verify all `unsafe` blocks have adequate `// SAFETY:` comments (currently present)
- [ ] T027 Run `cargo clippy -- -D warnings` and confirm zero warnings
- [ ] T028 Run `cargo doc --no-deps` and confirm zero warnings
- [ ] T029 Verify `cargo fmt --check` passes
- [ ] T030 Confirm criterion benchmarks produce stable results (CV < 15%) on target hardware
