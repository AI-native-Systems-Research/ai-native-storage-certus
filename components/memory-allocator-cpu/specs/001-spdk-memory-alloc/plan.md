# Implementation Plan: SPDK CPU Memory Allocator Component

**Branch**: `001-spdk-memory-alloc` | **Date**: 2026-04-10 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `/specs/001-spdk-memory-alloc/spec.md`

## Summary

Build a Certus component (`MemoryAllocatorCpu`) that exposes an `IMemoryManagement` interface for allocating, reallocating, zero-allocating, and freeing DMA-safe memory via SPDK's hugepage allocator. The component binds to the existing `ISPDKEnv` interface through a receptacle, uses the `DmaBuffer` type from the `interfaces` crate as the memory handle, and maintains thread-safe per-NUMA-zone allocation statistics. The implementation follows established patterns from `spdk-env` and `spdk-simple-block-device` components.

## Technical Context

**Language/Version**: Rust 1.75+ (edition 2021, workspace-inherited)
**Primary Dependencies**: component-framework, component-core, interfaces (with `spdk` feature), spdk-sys, spdk-env, example-logger
**Storage**: N/A (in-memory stats tracking only)
**Testing**: `cargo test` for unit tests, Criterion for benchmarks
**Target Platform**: Linux (SPDK/DPDK requires Linux hugepages and VFIO)
**Project Type**: library (Rust crate, Certus component)
**Performance Goals**: Allocation/free latency should be dominated by SPDK's own allocation cost; stats tracking overhead must be negligible (<1% of allocation time)
**Constraints**: Thread-safe (lock-based stats), SPDK env must be initialized first, hugepages required at runtime
**Scale/Scope**: Single crate in workspace, ~500-800 lines of implementation + tests + benchmarks

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

Constitution file contains template placeholders (not yet ratified). Deriving principles from project's stated goals in `info/PROMPTS.md`:

| Principle | Status | Notes |
|-----------|--------|-------|
| Code quality & maintainability | PASS | Follows established component patterns; idiomatic Rust |
| Extensive testing | PASS | Unit tests for all public APIs + edge cases planned |
| Linux-only | PASS | SPDK requires Linux; target platform is Linux |
| All public APIs must have unit tests | PASS | FR-013 requires full coverage |
| Rust doc tests for all public APIs | PASS | Will include doc examples on all interface methods |
| Criterion benchmarks for perf-sensitive code | PASS | FR-014 requires Criterion benchmarks |
| Code correctness assurance | PASS | Thread safety via Mutex; error handling for all failure modes |
| Component framework conformance | PASS | Uses define_interface!/define_component! macros |

No gate violations. Proceeding.

## Project Structure

### Documentation (this feature)

```text
specs/001-spdk-memory-alloc/
├── plan.md              # This file
├── research.md          # Phase 0: SPDK API research & decisions
├── data-model.md        # Phase 1: Entity & data model
├── quickstart.md        # Phase 1: Getting started guide
├── contracts/           # Phase 1: Interface contracts
│   └── imemory-management.md
└── tasks.md             # Phase 2: Task breakdown (created by /speckit.tasks)
```

### Source Code (repository root)

```text
components/memory-allocator-cpu/
├── Cargo.toml
├── src/
│   ├── lib.rs           # Component definition, IMemoryManagement impl
│   ├── error.rs         # MemoryAllocatorError type
│   └── stats.rs         # AllocationStats, ZoneStats types
├── benches/
│   └── allocator_bench.rs  # Criterion benchmarks
└── tests/               # Integration tests (if needed beyond unit tests)
```

**Structure Decision**: Single-crate library following the same flat structure as `spdk-env` and `spdk-simple-block-device`. Source is split into `lib.rs` (component + interface), `error.rs` (error type), and `stats.rs` (stats tracking). Benchmarks live in `benches/`.

## Key Technical Decisions

### 1. SPDK Function Mapping

| API | No NUMA affinity | With NUMA affinity |
|-----|-----------------|-------------------|
| `allocate` | `spdk_dma_zmalloc` (only zmalloc available in bindings) | `spdk_zmalloc` with `SPDK_MALLOC_DMA` flag |
| `zmalloc` | `spdk_dma_zmalloc` | `spdk_zmalloc` with `SPDK_MALLOC_DMA` flag |
| `free` | DmaBuffer's Drop calls `spdk_dma_free` or `spdk_free` | Same (free_fn stored in DmaBuffer) |
| `reallocate` | alloc new + copy + free old (no spdk_dma_realloc in bindings) | Same approach with NUMA-pinned alloc |

**Note**: The spdk-sys bindings expose `spdk_dma_zmalloc`, `spdk_dma_free`, `spdk_zmalloc`, and `spdk_free`. There is no `spdk_dma_malloc` (non-zero-init) or `spdk_dma_realloc` in the bindings. The `allocate` API will use `spdk_dma_zmalloc` / `spdk_zmalloc` since zero-initialization is the only available option. If a non-zero-init allocation is needed in the future, `spdk_dma_malloc` must be added to the spdk-sys bindings.

### 2. IMemoryManagement Interface Design

The interface must use `&self` (component framework constraint). The `free` method must consume the DmaBuffer (per clarification), but `define_interface!` methods take `&self`. Solution: `free` takes ownership of `DmaBuffer` by value as a parameter.

The `reallocate` method also consumes the old buffer and returns a new one.

### 3. Stats Lock Strategy

Stats are protected by a `Mutex<AllocationStatsInner>`. The lock is held only during stats counter updates (not during SPDK allocation calls), minimizing contention. The lock protects a `HashMap<i32, ZoneStats>` plus aggregate totals.

### 4. Workspace Integration

The new crate must be added to:
- Root `Cargo.toml` `[workspace.members]` and `[workspace.dependencies]`
- Not added to `default-members` (SPDK crates require pre-built native libs)

### 5. IMemoryManagement Interface in Interfaces Crate

The `IMemoryManagement` trait will be defined in the `interfaces` crate (under the `spdk` feature gate), following the same pattern as `ISPDKEnv` and `IBlockDevice`. This keeps interface definitions centralized. A new `MemoryAllocatorError` type will be added to `spdk_types.rs`.

## Complexity Tracking

No constitution violations to justify.
