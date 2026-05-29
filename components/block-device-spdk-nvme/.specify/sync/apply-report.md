# Sync Apply Report

**Date**: 2026-05-29
**Component**: block-device-spdk-nvme
**Triggered by**: drift-report.json (2026-05-29)

## Actions Taken

### Applied: FR-025 — ENOMEM retry backfill (approved)

**Change**: Added FR-025 to `specs/001-spdk-nvme-block-device/spec.md` specifying the ENOMEM retry loop behavior.

**File modified**:
- `specs/001-spdk-nvme-block-device/spec.md` (FR-025 added)

**New requirement**:
- **FR-025**: When an asynchronous NVMe command submission fails with ENOMEM (rc=-12), the actor retries for up to 50ms, polling all queue pairs for hardware completions on each iteration. On deadline expiry the operation fails with an error completion.

**Rationale**: The ENOMEM retry behavior in `src/actor.rs` (lines 618-670 and 718-773) is critical for multi-client correctness under queue pressure. Without it, concurrent cold lookups from multiple clients cause intermittent submission failures.

### Applied: FR-015 amendment — Queue pair depth tiering (approved)

**Change**: Amended FR-015 in `specs/001-spdk-nvme-block-device/spec.md` to formally specify the standard queue pair depths, selection heuristic, and io_queue_requests sizing.

**File modified**:
- `specs/001-spdk-nvme-block-device/spec.md` (FR-015 amended)

**Rationale**: The queue pair pool with `STANDARD_DEPTHS = [4, 16, 64, 256]`, shallowest-fit selection, and `io_queue_requests = depth * 2` sizing are important implementation details that affect performance characteristics and ENOMEM behavior under load.

## Post-apply State

- Specs analyzed: 2
- Requirements aligned: 50/50 (100%)
- Drifted: 0
- Unspecced features: 0
