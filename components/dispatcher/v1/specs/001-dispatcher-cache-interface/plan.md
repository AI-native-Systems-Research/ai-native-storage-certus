# Implementation Plan: Dispatcher Cache Interface

**Branch**: `001-dispatcher-cache-interface` | **Date**: 2026-04-28 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `/specs/001-dispatcher-cache-interface/spec.md`

## Summary

Implement the `IDispatcher` interface and `DispatcherError` type in the interfaces crate, then implement the full dispatcher component providing cache management (lookup, check, remove, populate) with GPU-to-SSD data flows via DMA staging buffers. The dispatcher coordinates N data block devices with N extent managers, handles MDTS-aware I/O segmentation, and performs asynchronous background writes from staging to SSD.

## Technical Context

**Language/Version**: Rust stable, edition 2021, MSRV 1.75  
**Primary Dependencies**: `component-framework`, `component-core`, `component-macros`, `interfaces` (with `spdk` feature)  
**Storage**: NVMe SSDs via SPDK (block-device-spdk-nvme), extent-manager for space allocation  
**Testing**: `#[test]` for unit/integration, Criterion for benchmarks, `cargo test --doc` for doc tests  
**Target Platform**: Linux only (RHEL/Fedora)  
**Project Type**: Library (Rust component crate, workspace member)  
**Performance Goals**: Minimize latency on lookup hot path; staging-to-SSD writes are async and off the critical path  
**Constraints**: 100ms fixed timeout for blocking operations; MDTS ~128 KiB I/O segmentation; SPDK DMA buffer allocation via `DmaBuffer::new`  
**Scale/Scope**: N data block devices + N extent managers per dispatcher instance; single metadata block device with N namespace partitions

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Status | Notes |
|-----------|--------|-------|
| I. Component-Framework Conformance | PASS | Uses `define_component!`, receptacles, lifecycle pattern |
| II. Interface-Only Public API | PASS | IDispatcher + DispatcherError defined in interfaces crate; no public functions outside component |
| III. Comprehensive Testing | PASS (planned) | Unit tests for all interface methods, doc tests, integration tests for wiring, edge case tests for concurrency |
| IV. Performance Assurance | PASS (planned) | Criterion benchmarks for lookup, populate, check hot paths |
| V. Documentation & Maintainability | PASS (planned) | Doc comments with examples on all public API items; `cargo doc --no-deps` must be warning-free |
| VI. Code Quality & Correctness | PASS (planned) | `cargo fmt --check`, `cargo clippy -- -D warnings`, idiomatic Result/Option |
| VII. Linux-Only Platform | PASS | No cross-platform concerns; SPDK targets Linux kernel interfaces |

No violations. No complexity tracking entries needed.

## Project Structure

### Documentation (this feature)

```text
specs/001-dispatcher-cache-interface/
├── plan.md              # This file
├── research.md          # Phase 0 output
├── data-model.md        # Phase 1 output
├── quickstart.md        # Phase 1 output
├── contracts/           # Phase 1 output
│   ├── idispatcher.md   # IDispatcher interface contract
│   └── errors.md        # DispatcherError type contract
└── tasks.md             # Phase 2 output (created by /speckit.tasks)
```

### Source Code (repository root)

```text
components/interfaces/src/
├── idispatcher.rs          # NEW: IDispatcher interface + DispatcherError type
└── lib.rs                  # MODIFY: add idispatcher module + re-exports

components/dispatcher/v0/
├── src/
│   ├── lib.rs              # MODIFY: define_component! + IDispatcher impl
│   ├── io_segmenter.rs     # NEW: MDTS-aware I/O splitting logic
│   └── background.rs       # NEW: async staging-to-SSD write worker
├── benches/
│   └── dispatcher_benchmark.rs  # NEW: Criterion benchmarks
└── Cargo.toml              # MODIFY: add dev-dependencies for benchmarks
```

**Structure Decision**: Single-crate component following the dispatch-map/extent-manager pattern. Internal modules split by concern: `io_segmenter.rs` for MDTS-aware block I/O splitting, `background.rs` for the async write worker thread. The main `lib.rs` holds the component definition, interface implementation, and tests. This matches the convention where components keep their code in a small number of focused modules.

## Architecture Decisions

### AD-1: Handling N Block Devices and N Extent Managers

The component framework's receptacles are single-slot (`Receptacle<T>` holds one `Arc<T>`). The functional design requires N data block devices and N extent managers where N is runtime-determined.

**Decision**: The `IDispatcher::initialize()` method accepts configuration parameters (PCI addresses) and the dispatcher internally creates and manages the N block device and extent manager component instances. Single shared dependencies (logger, dispatch_map) remain as receptacles.

**Rationale**: This is consistent with the functional design ("PCI addresses are passed as a parameter to the initialization function"). The dispatcher acts as a higher-level coordinator that owns and manages its data-path components.

**Receptacle layout**:
- `logger: ILogger` — shared logging (optional, best-effort)
- `dispatch_map: IDispatchMap` — cache state tracking

The `block_device_admin` receptacle from the skeleton is removed; block devices are created internally during initialize.

### AD-2: Background Write Worker

**Decision**: Follow the extent-manager pattern — `std::thread::spawn` with `Arc<AtomicBool>` shutdown flag. A single background thread drains a channel of pending staging-to-SSD write jobs. The `JoinHandle` is stored in `Mutex<Option<JoinHandle<()>>>` for clean shutdown.

**Rationale**: Simpler than the actor pattern (no SPDK thread affinity needed for the coordinator), and the extent-manager has proven this pattern works well. The background thread interacts with block devices via their channel-based client API.

### AD-3: MDTS I/O Segmentation

**Decision**: The dispatcher queries `IBlockDevice::max_transfer_size()` at initialization and splits all reads/writes into segments of at most that size. Segments are submitted via `Command::BatchSubmit` for efficient batching.

**Rationale**: The block device component does not auto-split (confirmed by research). The dispatcher is the right layer for this since it knows the full transfer size before submission.

### AD-4: Concurrency Model

**Decision**: Rely on the dispatch map's built-in read/write reference locking (`take_read`, `take_write`, `release_read`, `release_write`, `downgrade_reference`). The dispatcher does not add its own locking layer on top.

**Rationale**: The `IDispatchMap` interface already provides correct concurrent access semantics with blocking on writer contention. Adding a second lock layer would risk deadlocks and add unnecessary complexity.

### AD-5: Error Propagation from Background Writes

**Decision**: Background write failures (I/O error, out of space) remove the dispatch map entry and free the staging buffer. The error is logged via the logger receptacle. Since the background write is asynchronous and the caller has already received confirmation from populate(), there is no synchronous error channel back to the caller.

**Rationale**: Per spec clarification — errors are raised (logged), entry removed, staging freed. The caller's populate() already returned success for the staging phase; the background failure is an operational concern surfaced through logging.

## Complexity Tracking

No violations to justify.
