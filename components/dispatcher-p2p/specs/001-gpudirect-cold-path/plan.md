# Implementation Plan: GPUDirect Storage Cold Path

**Branch**: `p2p_component` | **Date**: 2026-06-11 | **Spec**: [spec.md](spec.md)

**Input**: Feature specification from `specs/001-gpudirect-cold-path/spec.md`

## Summary

Add a GPUDirect Storage cold-read path to the dispatcher-p2p component. When cached entries have been evicted from DRAM to NVMe SSD, the P2P path reads data using NVMe DMA directly into pre-allocated GPU BAR1 staging buffers, then issues D2D copies to the client's GPU destination. This eliminates the host DRAM bounce. The component falls back to the standard DRAM path when P2P hardware is unavailable.

## Technical Context

**Language/Version**: Rust stable, edition 2021, MSRV 1.75

**Primary Dependencies**: gpu-services (p2p feature), block-device-spdk-nvme, extent-manager, memory-tier, spdk-env, component-framework

**Storage**: NVMe SSDs via SPDK userspace drivers (extent-based allocation)

**Testing**: `cargo test -p dispatcher-p2p` (unit + doc tests), `certus-api-bench_v2.py` (end-to-end performance)

**Target Platform**: Linux only (RHEL/Fedora)

**Project Type**: Library component (Rust crate implementing IDispatcher interface)

**Performance Goals**: Measurable throughput difference between P2P and DRAM cold paths via Criterion benchmarks and certus-api-bench_v2.py

**Constraints**: Must be drop-in replacement for standard dispatcher; P2P failure must not crash the component; 64-slot staging ring shared across threads

**Scale/Scope**: 4+ concurrent clients, 64 staging ring slots, effective queue depth 16 per thread

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Status | Notes |
|-----------|--------|-------|
| I. Component-Framework Conformance | PASS | Uses `define_component!`, receptacles for all deps, implements IUnknown |
| II. Interface-Only Exposure | PASS | Exposes only IDispatcher from components/interfaces crate |
| III. Code Quality and Correctness | PASS | Linux-only, clippy/fmt/doc gates, unsafe with SAFETY comments |
| IV. Comprehensive Testing | PASS | Unit tests, doc tests, mock-based tests for CI without hardware |
| V. Performance Measurement | PASS | Criterion benchmarks + certus-api-bench_v2.py for comparison |
| VI. Documentation Standards | PASS | Doc comments on all public APIs, module-level docs |
| VII. Maintainability and Graceful Degradation | PASS | DRAM fallback on P2P failure, YAGNI, explicit error handling |

## Project Structure

### Documentation (this feature)

```text
specs/001-gpudirect-cold-path/
├── plan.md              # This file
├── research.md          # Phase 0 output
├── data-model.md        # Phase 1 output
├── quickstart.md        # Phase 1 output
├── contracts/           # Phase 1 output
└── tasks.md             # Phase 2 output (/speckit-tasks)
```

### Source Code (component root)

```text
src/
├── lib.rs               # define_component!, IDispatcher impl, path selection
├── p2p_ring.rs          # P2pRing: 64-slot GPU BAR1 staging ring allocation/cleanup
├── pipeline.rs          # PipelineRing (DRAM path) + pipelined_ssd_to_gpu_p2p (P2P path)
├── background.rs        # BackgroundEvictor, ParallelBackgroundWriter
└── io_segmenter.rs      # I/O chunking utility

benches/
└── cold_path_benchmark.rs  # Criterion benchmarks for P2P vs DRAM cold paths

tests/
└── integration/         # Mock-based integration tests
```

**Structure Decision**: Single Rust crate mirroring the existing standard dispatcher layout, with `p2p_ring.rs` added for the new P2P staging ring and pipeline modifications in `pipeline.rs`.

## Complexity Tracking

No constitution violations to justify.
