# Spec Apply Report

Generated: 2026-05-29
Project: certus-server
Based on: drift-report.md (2026-05-29)

## Summary

| Action | Count |
|--------|-------|
| Requirements Added (Backfill) | 2 |
| Requirements Updated (Backfill) | 2 |
| Requirements Left for Human Decision | 1 |
| Spec File Modified | specs/001-grpc-dispatcher-server/spec.md |

## Applied Changes

### 1. FR-011 Updated — IpcHandle gains gpu_device_id field

**Direction**: BACKFILL (code -> spec)
**Status**: APPLIED

**Before**: FR-011 described IpcHandle as containing a 64-byte CUDA IPC handle and a uint32 size only.

**After**: FR-011 now specifies three fields: (1) `bytes cuda_ipc_handle` (64-byte opaque blob), (2) `uint32 size` (data size in bytes), (3) `int32 gpu_device_id` (CUDA device ordinal). The requirement now states that the client MUST populate `gpu_device_id` to enable correct `cudaSetDevice` behavior on the server.

**Rationale**: The proto field was added to fix multi-GPU correctness. The spec was silent on this field entirely, leaving clients without guidance on whether to populate it.

---

### 2. FR-018 Added — Global persistent IPC handle cache

**Direction**: BACKFILL (code -> spec)
**Status**: APPLIED

**Before**: No requirement covered the global IPC cache. FR-003 mentioned within-batch handle deduplication only, which is a narrower and qualitatively different property.

**After**: FR-018 specifies the global process-lifetime `IpcCache` structure (keyed by 64-byte handle bytes, storing `dev_ptr`, `gpu_device_id`, and `refcount`), the open/cache/increment protocol, the decrement/close/evict protocol, and the motivation (eliminates "resource already mapped" errors; removes CUDA IPC lock serialization).

**Rationale**: This is a significant architectural change to the IPC lifecycle. It affects how clients reason about handle lifetimes and how concurrent requests interact. Speccing it correctly prevents future regressions.

---

### 3. FR-019 Added — cudaSetDevice before cudaIpcOpenMemHandle

**Direction**: BACKFILL (code -> spec)
**Status**: APPLIED

**Before**: No requirement covered CUDA device selection before opening IPC handles. The CUDA IPC mechanism requires the server to be on the correct device, but this was entirely absent from the spec.

**After**: FR-019 specifies that the server MUST call `cudaSetDevice(gpu_device_id)` before `cudaIpcOpenMemHandle` for any uncached handle when `gpu_device_id >= 0`. It further specifies that a `cudaSetDevice` failure MUST propagate as an `IoError` for that entry. The `gpu_device_id < 0` sentinel case (meaning "not specified") is also covered.

**Rationale**: Without this requirement, implementations could omit the `cudaSetDevice` call and produce subtly wrong behavior on multi-GPU machines where the server CUDA context is not on the same device as the allocated memory.

---

### 4. Key Entities IpcHandle Updated

**Direction**: BACKFILL (code -> spec)
**Status**: APPLIED

**Before**: "Opaque handle to client GPU memory containing a 64-byte CUDA IPC memory handle and a size (uint32)."

**After**: Updated to list all three fields including `gpu_device_id: int32` and cross-references FR-019.

---

### 5. Clarifications Session Added (2026-05-29)

**Direction**: DOCUMENTATION
**Status**: APPLIED

Added three Q&A entries covering the IPC handle cache design rationale, the `cudaSetDevice` requirement, and the `gpu_device_id` proto field addition.

---

## Deferred (Human Decision Required)

### FR-009 — SIGTERM handling not implemented

**Finding**: Spec requires SIGTERM/SIGINT handling; code only catches SIGINT via `tokio::signal::ctrl_c()`.

**Why Deferred**: This is a code bug relative to an existing spec requirement — the spec is correct and the code needs to be fixed. This is out of scope for a backfill pass (which only updates spec to match code). A separate issue should be filed to add SIGTERM handling.

**Suggested Fix**: Use `tokio::signal::unix::{signal, SignalKind}` to also catch `SIGTERM` alongside `ctrl_c()`.
