# Sync Apply Report

Applied: 2026-09-09

## Changes Made

### Specs Updated

| Spec | Requirement | Change Type | Direction |
|------|-------------|-------------|-----------|
| 001-gpu-cuda-services | FR-015 | Modified (appended) | Backfill |
| 001-gpu-cuda-services | FR-020 | Modified (appended) | Backfill |
| 002-gpu-ssd-dma-prepare | FR-019 | Modified (appended) | Backfill |

All three backfills document the `ecf2dbe3` (2026-09-02) Gate #1 change —
host memory is now registered/allocated with the CUDA **portable** flag
(`CUDA_HOST_REGISTER_PORTABLE` / `CUDA_HOST_ALLOC_PORTABLE`) so a shared pool
stays pinned/zero-copy across all GPU contexts for multi-GPU data-parallel
serving — plus the pre-existing idempotent registration handling
(`ALREADY_REGISTERED` / SPDK `EBUSY` treated as success; conditional
`cudaHostUnregister` rollback).

### New Specs Created

None.

### Implementation Tasks Generated

None (no ALIGN proposals — all resolutions were spec backfills; the code is
authoritative and unchanged).

### Not Applied

None — all three proposals were approved and applied.

## Backups

- `.specify/sync/backups/spec-001-<ts>.md.bak`
- `.specify/sync/backups/spec-002-<ts>.md.bak`

## Result

No actionable spec/implementation drift remains. Drift report stamped
`spec_sync_drift_status: clean`.

## Next Steps

1. (Optional) A human maintainer pass on spec 003 to graduate it from
   backfilled-draft ("needs human review") status.
2. Commit specs + reports together so the freshness stamp travels with the
   inputs it certifies.
