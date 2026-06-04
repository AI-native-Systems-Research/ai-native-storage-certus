# Sync Apply Report

Generated: 2026-05-29
Component: gpu-services
Based on: drift-report from 2026-05-29

## Applied Changes

### 1. Backfill open_ipc_handle Caller Precondition (spec 001)

**File modified**: `specs/001-gpu-cuda-services/spec.md`

**Change 1 — Assumptions section (last bullet, replaced)**:

Removed the inaccurate assumption that "GPU device selection for IPC
operations is implicit — the IPC handle carries the originating device
context and the component follows it automatically."

Replaced with accurate language documenting that:
- `open_ipc_handle` (and `deserialize_ipc_handle`) does NOT call
  `cudaSetDevice`
- It is a low-level function with an explicit caller precondition:
  the caller must set the CUDA device context via `cudaSetDevice` before
  calling `deserialize_ipc_handle`
- In the certus-server integration, `service.rs` fulfills this
  precondition
- The high-level `prepare_memory_for_spdk` (FR-013) handles device
  context internally and is exempt from this requirement

**Change 2 — FR-003 (augmented with precondition note)**:

Added a **Precondition** clause to FR-003 stating that the caller is
responsible for setting the correct CUDA device context before calling
`deserialize_ipc_handle`, and that `open_ipc_handle` does not call
`cudaSetDevice` internally. The high-level `prepare_memory_for_spdk`
path is identified as exempt.

**Effect**: The drift between the Assumptions section and actual code
behavior of `src/ipc.rs open_ipc_handle()` is now resolved. No code
changes required — the code is correct, the spec was inaccurate.

## Not Applied

None. The single drift item has been fully addressed by spec backfill.

## Post-Apply Drift Status

| Category | Before | After |
|----------|--------|-------|
| Aligned (requirements) | 43 | 44 |
| Drifted | 1 | 0 |
| Not Implemented | 0 | 0 |
| Unspecced | 0 | 0 |

## Notes

The previous sync (2026-05-21) had already resolved all prior drift and
unspecced features. This sync addresses a single documentation-level
drift: an inaccurate Assumption about device context selection for
`open_ipc_handle`. The correction clarifies the architectural boundary
between the low-level IPC path (caller-managed device context) and the
high-level `prepare_memory_for_spdk` path (self-managing device
context). This is particularly important for future callers who use
`deserialize_ipc_handle` directly in multi-GPU scenarios.
