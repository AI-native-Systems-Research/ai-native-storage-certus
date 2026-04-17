# Implementation Plan: Extent Management

**Branch**: `001-extent-management` | **Date**: 2026-04-16 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `/specs/001-extent-management/spec.md`

## Summary

Implement a fixed-size storage extent manager component (ExtentManagerComponentV1) that allocates, tracks, and reclaims fixed-size extents on NVMe SSDs with crash-consistent metadata. The component exposes IExtentManager and IExtentManagerAdmin interfaces via the component-framework, persists metadata through an IBlockDevice receptacle using a two-phase atomic write protocol, and supports up to 32 size classes with up to 10 million slots each.

## Technical Context

**Language/Version**: Rust stable, edition 2021, MSRV 1.75
**Primary Dependencies**: component-framework (component-core, component-macros) at `../../component-framework/crates/`; interfaces at `../../interfaces`; `crc32fast` for CRC-32 checksums; `criterion` for benchmarks
**Storage**: NVMe SSD via IBlockDevice receptacle; 4KiB-aligned atomic writes; on-disk layout: superblock + bitmap region + extent record region
**Testing**: `cargo test -p extent-manager`; MockBlockDevice (in-memory `HashMap<u64, [u8; 4096]>` with actor threads); FaultConfig for crash injection; `heap_dma_alloc()` for test allocations
**Target Platform**: Linux only
**Project Type**: Library (component-framework component)
**Performance Goals**: Iteration sufficient for in-memory index rebuild at startup; single-extent operations in microseconds for mock device
**Constraints**: No SPDK/hugepages required for tests; no nightly features; no external runtime dependencies beyond component-framework
**Scale/Scope**: 1-32 size classes (128KiB-5MiB), up to 10,000,000 slots per size class

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Status | Evidence |
|-----------|--------|----------|
| 1. Correctness First | PASS | Unit tests for all public APIs; crash recovery tests with fault injection at every write boundary; unsafe code justified and tested |
| 2. Comprehensive Testing | PASS | Unit tests in src/*.rs, integration tests in tests/ (api_operations, crash_recovery, thread_safety), doc tests on all public items; MockBlockDevice for hardware-free CI |
| 3. Performance Accountability | PASS | Criterion benchmarks for create, remove, lookup, iterate, recovery/open operations |
| 4. Documentation as Contract | PASS | All public types/functions/methods get rustdoc with examples; `cargo doc --no-deps` zero warnings |
| 5. Maintainability | PASS | `pub(crate)` default; cargo fmt + clippy enforced; minimal public surface |
| 6. Component-Framework Conformance | PASS | `define_component!` macro; IBlockDevice + ILogger receptacles; IExtentManager + IExtentManagerAdmin providers; standard lifecycle |
| 7. Interface-Driven Public API | PASS | All public functions via IExtentManager/IExtentManagerAdmin trait implementations only |
| 8. Platform and Toolchain Discipline | PASS | Linux only, stable toolchain, edition 2021, MSRV 1.75, no nightly, no external deps beyond component-framework |

**Gate result**: ALL PASS. No violations.

## Project Structure

### Documentation (this feature)

```text
specs/001-extent-management/
├── plan.md              # This file
├── research.md          # Phase 0 output
├── data-model.md        # Phase 1 output
├── quickstart.md        # Phase 1 output
├── contracts/           # Phase 1 output
│   └── interfaces.md    # IExtentManager + IExtentManagerAdmin contracts
└── tasks.md             # Phase 2 output (/speckit.tasks command)
```

### Source Code (repository root)

```text
Cargo.toml
src/
├── lib.rs               # define_component!, ExtentManagerComponentV1, re-exports
├── bitmap.rs            # AllocationBitmap — per-size-class bitmap management
├── block_device.rs      # BlockDevice wrapper — typed read/write through IBlockDevice
├── error.rs             # Local error types (maps to ExtentManagerError)
├── metadata.rs          # ExtentMetadata, OnDiskExtentRecord, serialization
├── recovery.rs          # Crash recovery logic — scan records vs bitmaps on open()
├── superblock.rs        # Superblock — on-disk header with magic, version, config, CRC
├── state.rs             # ExtentManagerState — runtime state (index, bitmaps, config)
└── test_support.rs      # MockBlockDevice, FaultConfig, heap_dma_alloc(), create_test_component() [cfg(any(test, feature="testing"))]

tests/
├── api_operations.rs    # Integration tests for create/remove/lookup/iterate
├── crash_recovery.rs    # Fault injection tests — power failure at each write phase
└── thread_safety.rs     # Concurrent access tests — multi-threaded mixed operations

benches/
├── create_benchmark.rs  # Criterion: single and bulk extent creation
├── remove_benchmark.rs  # Criterion: single and bulk extent removal
├── lookup_benchmark.rs  # Criterion: extent lookup by key
└── iterate_benchmark.rs # Criterion: full iteration over all extents
```

**Structure Decision**: Single-crate Rust library following the v0 module layout pattern. Modules map 1:1 to on-disk structure concepts (superblock, bitmap, metadata, recovery). State management extracted to its own module for clarity at 10M-slot scale.
