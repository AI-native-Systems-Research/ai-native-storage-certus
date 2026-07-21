# Spec Sync Apply Report
Applied: 2026-07-21
Project: gpu-services
Based on: proposals from 2026-07-21

## Summary

| Item | Value |
|------|-------|
| Target spec | 001-gpu-cuda-services |
| Proposals applied | 2 (both BACKFILL) |
| FRs added | FR-021, FR-022 |
| Spec files modified | specs/001-gpu-cuda-services/spec.md |
| Backup | .specify/sync/backups/spec.md.bak |

## Changes Applied

### FR-021 - set_device (BACKFILL, HIGH confidence)
Added after FR-020 in spec 001-gpu-cuda-services. Documents the existing
`set_device(device)` method (src/lib.rs:566-592,
interfaces/src/igpu_services.rs:555) that binds the calling thread's CUDA
device context via `cudaSetDevice`.

### FR-022 - device_of_ptr (BACKFILL, HIGH confidence)
Added after FR-021 in spec 001-gpu-cuda-services. Documents the existing
`device_of_ptr(ptr)` method (src/lib.rs:594-633,
interfaces/src/igpu_services.rs:577) that returns the owning CUDA device
ordinal via `cudaPointerGetAttributes`, returning -1 when no device
association exists.

## Before / After

- Before: spec 001 Functional Requirements ended at FR-020 (20 FRs).
- After: spec 001 Functional Requirements end at FR-022 (22 FRs). FR-021 and
  FR-022 inserted between FR-020 and the "Key Entities" section. No existing
  FR text was modified. Numbering and Markdown format match surrounding FRs.

## Verification
- Backup created at .specify/sync/backups/spec.md.bak before edit.
- proposals.json: both proposals set to "approved": true.
- No other spec (002-gpu-ssd-dma-prepare) was modified.
