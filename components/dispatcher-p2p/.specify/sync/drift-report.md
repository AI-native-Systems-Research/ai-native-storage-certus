# Drift Report: dispatcher-p2p (001-gpudirect-cold-path)

**Generated**: 2026-06-15  
**Spec**: `components/dispatcher-p2p/specs/001-gpudirect-cold-path/spec.md`  
**Implementation**: `components/dispatcher-p2p/src/` (lib.rs, pipeline.rs, p2p_ring.rs)

## Summary

| Status | Count |
|--------|-------|
| Aligned | 11 |
| Drifted (minor) | 1 |
| Resolved prior session | 4 |

## Current Drift

### DRIFT-A: P2P ring failure panics on first cold lookup, not at startup (Severity: Minor)

**Spec says** (FR-006): "System MUST panic at startup if the P2P ring cannot be initialized"

**Code does** (lib.rs:965-978): Init logs warning and continues. First cold lookup panics via `expect()` (lib.rs:1450).

**Impact**: Hot-only workloads proceed without P2P hardware; cold lookup triggers panic. Current behavior is arguably more flexible for testing.

**Recommendation**: Accept current behavior and update FR-006 to: "System MUST panic on first cold lookup if the P2P ring was not initialized. Initialization logs a warning but does not fail, allowing hot-only testing without P2P hardware."

## Changes Applied (2026-06-12)

### RESOLVED: DRAM fallback removed → fail-fast at startup

- User Story 2 rewritten: now "Fail Fast When P2P Unavailable"
- FR-006: panic instead of fallback
- FR-007: no runtime path selection
- SC-006: panic diagnostic instead of fallback logging

### RESOLVED: P2P ring uses real BAR1 (not pinned host memory)

- FR-003 updated: specifies cudaMalloc + GDRCopy + spdk_mem_register

### RESOLVED: Pipeline sync strategy aligned with manual branch

- FR-005 updated: FIFO ordering, sync only on slot recycle, no final sync

### RESOLVED: Performance measurement references standard dispatcher for comparison

- US4 scenario 2 and SC-005 reference full.yaml instead of "DRAM fallback"

## Measured Performance (2026-06-12)

- P2P cold, 1 client, 6 drives: **9.31 GB/s** (with promotion)
- P2P cold, 4 clients, 6 drives: **6.43 GB/s** aggregate (with promotion)
- BAR1 theoretical ceiling at 128 KiB: 10 GB/s
- Non-P2P DRAM bounce (full.yaml): 15.5 GB/s (1c), 12.2 GB/s (4c aggregate)
