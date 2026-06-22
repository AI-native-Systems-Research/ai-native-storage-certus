# Spec Drift Report

Generated: 2026-06-18
Project: block-device-filesys
Spec: 001-block-device-filesys

## Summary

| Category | Count |
|----------|-------|
| Specs Analyzed | 1 |
| Requirements Checked | 18 |
| Aligned | 16 (89%) |
| Drifted | 1 (5.5%) |
| Not Implemented | 1 (5.5%) |
| Unspecced Code | 1 |

## Detailed Findings

### Spec: 001-block-device-filesys - Block Device Filesys Component

#### Aligned

- FR-001: IBlockDevice implemented  `src/lib.rs:224`
- FR-002: ILogger receptacle declared  `src/lib.rs:59`
- FR-003: `define_component!` macro used  `src/lib.rs:54`
- FR-005: Uses regular file on Linux filesystem  `src/config.rs:114`
- FR-006: Configurable block size (default 512) and num_blocks  `src/config.rs:58`
- FR-007: ReadSync/WriteSync via pread/pwrite + fdatasync  `src/actor.rs:213,274`
- FR-008: ReadAsync/WriteAsync via io_uring with timeout and OpHandle  `src/actor.rs:335,450`
- FR-009: WriteZeros with fdatasync  `src/actor.rs:582`
- FR-010: BatchSubmit processes sequentially  `src/actor.rs:186`
- FR-011: AbortOp implemented via AsyncCancel  `src/actor.rs:657`
- FR-012: NsProbe returns single namespace  `src/actor.rs:672`
- FR-013: Actor model with dedicated thread, io_uring event loop  `src/actor.rs:811`
- FR-014: Criterion benchmarks present  `benches/latency.rs`, `benches/throughput.rs`
- FR-016: Backing file created via fallocate; size mismatch errors  `src/config.rs:118-167`
- FR-017: DmaBuffer byte slices accessed directly  `src/actor.rs:245,294`
- FR-018: `io-uring` crate dependency present  `Cargo.toml`

#### Drifted

- FR-004: Spec says "MUST NOT expose any public functions outside its interface definitions" but code exposes `pub mod config` (making `DeviceConfig`, `open_or_create_backing_file` public) and `pub use` of interface types.
  - Location: `src/lib.rs:27`, `src/lib.rs:41-44`
  - Severity: minor
  - Note: The `config` module is marked `pub` giving external access to `DeviceConfig::new()` and `open_or_create_backing_file()`. The re-exports of interface types are convenience re-exports and arguably acceptable, but `config` module internals are not interface methods.

#### Not Implemented

- FR-015: "All public API items MUST have documentation tests." — The `BlockDeviceFilesysComponent::create()` method uses `/// ```ignore` rather than a runnable doc test. The integration test file covers functionality but `cargo doc --test` would not exercise these examples.

### Unspecced Code

| Feature | Location | Lines | Suggested Spec |
|---------|----------|-------|----------------|
| O_DIRECT + O_SYNC file open with fallback | `src/config.rs:171-197` | 27 | Update FR-007/FR-008 |

The spec mentions "fdatasync after each write" for durability but the implementation opens the file with O_SYNC (making fdatasync redundant for sync writes) and O_DIRECT (bypassing page cache). This is an enhancement beyond the spec that changes durability semantics — O_SYNC guarantees write-through without needing explicit fdatasync.

## Inter-Spec Conflicts

None detected.

## Recommendations

1. **FR-004 drift**: Either restrict `config` module to `pub(crate)` visibility, or update the spec to acknowledge that `DeviceConfig` is intentionally part of the public API for external configuration use cases.
2. **FR-015 gap**: Convert `/// ```ignore` examples to runnable doc tests (may require `tempfile` in dev-dependencies or conditional compilation).
3. **Unspecced O_DIRECT/O_SYNC**: Update spec to document O_DIRECT + O_SYNC open strategy with tmpfs fallback, since this meaningfully changes the durability and caching model beyond what "fdatasync after each write" describes.
