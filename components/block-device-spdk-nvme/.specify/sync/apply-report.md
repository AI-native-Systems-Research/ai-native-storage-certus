# Sync Apply Report

**Date**: 2026-05-21
**Component**: block-device-spdk-nvme
**Triggered by**: drift-report.json (2026-05-21)

## Actions Taken

### Applied: P7 — Naming backfill (auto-approved)

**Change**: Renamed `BlockDeviceSpdkNvmeComponentV1` to `BlockDeviceSpdkNvmeComponent` in spec artifacts.

**File modified**:
- `specs/002-iops-benchmark/tasks.md` (line 59, T016): `BlockDeviceSpdkNvmeComponentV1` -> `BlockDeviceSpdkNvmeComponent`

**Rationale**: The source code (`src/lib.rs`) defines the component as `BlockDeviceSpdkNvmeComponent` (no version suffix). The V1 suffix in the spec was a stale reference from before the component was renamed. This is a cosmetic alignment with no functional impact.

**Verification**: The string `BlockDeviceSpdkNvmeComponentV1` no longer appears in any spec file under `specs/`.

### Applied: P8 — Relax SC-008 (benchmarks)

**Change**: Updated SC-008 in `specs/001-spdk-nvme-block-device/spec.md` to state that performance measurement is satisfied by the dedicated IOPS benchmark application (`apps/iops-benchmark`) rather than requiring per-function Criterion benchmarks within this crate.

**File modified**:
- `specs/001-spdk-nvme-block-device/spec.md` (SC-008)

**Rationale**: The crate does not have a `benches/` directory. Performance-sensitive paths (IO submission, batch processing, qpair selection) are exercised under realistic workloads by the `apps/iops-benchmark` application, which provides configurable queue depths, block sizes, thread counts, and access patterns. This is a more representative measurement approach than isolated micro-benchmarks for a hardware-bound driver.

### Applied: P9 — Unspecced IOPS benchmark features

**Change**: Added three new functional requirements to `specs/002-iops-benchmark/spec.md` for features present in the implementation but previously unspecified.

**File modified**:
- `specs/002-iops-benchmark/spec.md` (FR-023, FR-024, FR-025)

**New requirements**:
- **FR-023**: Comma-separated `--block-size` for mixed-size workloads (each IO randomly picks one of the provided sizes)
- **FR-024**: `--batch-size` flag for grouping IOs into BatchSubmit messages
- **FR-025**: NUMA-aware worker thread pinning to cores in the same NUMA zone as the target NVMe controller

**Rationale**: These features exist in the `apps/iops-benchmark` implementation and are important for realistic benchmarking (mixed workloads, batch submission efficiency, and NUMA-local execution). Formalizing them ensures they are maintained and tested.

## Post-apply State

- Specs analyzed: 2
- Requirements aligned: 48/48 (100%)
- Drifted: 0
- Unspecced features: 0
