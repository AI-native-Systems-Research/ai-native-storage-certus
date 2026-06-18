# Drift Report: dispatcher-p2p (001-gpudirect-cold-path)

**Generated**: 2026-06-16  
**Spec**: `components/dispatcher-p2p/specs/001-gpudirect-cold-path/spec.md`  
**Implementation**: `components/dispatcher-p2p/src/` (lib.rs, pipeline.rs, p2p_ring.rs)

## Summary

| Status | Count |
|--------|-------|
| Aligned | 13 |
| Drifted | 0 |
| Resolved this session | 3 |
| Resolved prior session | 4 |

## Current Drift

No active drift. All requirements aligned with implementation.

## Resolved This Session (2026-06-16)

### DRIFT-A: P2P ring failure behavior — RESOLVED (was Minor)

FR-006 already aligned — spec and code both specify panic on first cold lookup, not at startup.

### DRIFT-B: `promote_to_memory_tier` unspecced — RESOLVED

Added FR-013 to spec: "System MUST implement `promote_to_memory_tier(keys)` to asynchronously read cold entries from NVMe into the memory-tier without GPU involvement."

### DRIFT-C: Thread topology and CUDA streams — RESOLVED

Updated FR-004 (thread partition with `MAX_QUEUES_PER_DRIVE=1`) and FR-005 (4 CUDA streams, round-robin D2D, sync interval = ring_size, final sync). Added "Thread Partition" to Key Entities.

---

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

