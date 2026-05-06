# Drift Resolution Proposals

Generated: 2026-05-05
Based on: drift-report from 2026-05-05

## Summary

| Resolution Type | Count |
|-----------------|-------|
| Backfill (Code -> Spec) | 1 |
| Align (Spec -> Code) | 1 |
| Human Decision | 0 |
| New Specs | 0 |
| Remove from Spec | 0 |

## Proposals

### Proposal 1: 001-grpc-dispatcher-server/FR-009

**Direction**: ALIGN (Spec -> Code)

**Current State**:
- Spec says: "Server MUST shut down the dispatcher when receiving SIGTERM/SIGINT, draining active requests and completing all in-flight operations before exiting"
- Code does: "Only SIGINT handled via `tokio::signal::ctrl_c()`; SIGTERM not caught, so `kill <pid>` does not trigger graceful shutdown"

**Proposed Resolution**:

Add SIGTERM handling alongside the existing SIGINT handler. Replace the shutdown future with:

```rust
use tokio::signal::unix::{signal, SignalKind};

async move {
    let mut sigterm = signal(SignalKind::terminate()).expect("failed to register SIGTERM handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {},
        _ = sigterm.recv() => {},
    }
    flag_clone.store(true, Ordering::Release);
    eprintln!("\ncertus-server: shutting down...");
}
```

**Rationale**: The spec explicitly requires both SIGTERM and SIGINT handling. SIGTERM is the standard signal sent by `kill`, `systemd`, and container orchestrators (Docker/Kubernetes) during shutdown. Without it, production deployments would force-kill the process, risking data corruption in the dispatcher. The spec correctly identifies this as a requirement — the code has a gap.

**Confidence**: HIGH

**Action**:
- [ ] Approve
- [ ] Reject
- [ ] Modify

---

### Proposal 2: 001-grpc-dispatcher-server/FR-011

**Direction**: BACKFILL (Code -> Spec)

**Current State**:
- Spec says: "IPC handle MUST be represented as a serializable structure containing a memory address (or opaque identifier) and a size field"
- Assumptions section says: "The IPC handle's `address` field is transmitted as a 64-bit integer (pointer value) since client and server share the same address space for GPU DMA"
- Code does: "IPC handle uses a 64-byte CUDA IPC memory handle (`bytes cuda_ipc_handle`) plus `uint32 size`. Server calls `cudaIpcOpenMemHandle` to obtain a local device pointer, then `cudaIpcCloseMemHandle` after the operation."

**Proposed Resolution**:

Update FR-011 to:

> **FR-011**: The IPC handle in the gRPC protocol MUST be represented as a serializable structure containing a CUDA IPC memory handle (64-byte opaque blob from `cudaIpcGetMemHandle`) and a size field (uint32, data size in bytes). The server opens the IPC handle to obtain a device pointer in its own CUDA context, performs the operation, then closes the handle.

Remove the assumption:

> ~~The IPC handle's `address` field is transmitted as a 64-bit integer (pointer value) since client and server share the same address space for GPU DMA.~~

Replace with:

> The IPC handle uses CUDA's cross-process memory sharing (`cudaIpcGetMemHandle`/`cudaIpcOpenMemHandle`). Client and server may be separate processes; they share GPU memory via the CUDA IPC mechanism rather than raw pointer values.

**Rationale**: The implementation correctly uses CUDA IPC handles, which is the standard mechanism for cross-process GPU memory sharing. The original spec assumption about "same address space" was incorrect — gRPC implies separate processes, so raw pointers won't work. The code evolved to the correct design. The spec should be updated to document the actual (and correct) mechanism.

**Confidence**: HIGH

**Action**:
- [ ] Approve
- [ ] Reject
- [ ] Modify

---

### Proposal 3: Unspecced Feature - CUDA IPC handle open/close

**Direction**: NO_ACTION (covered by Proposal 2)

**Feature**: CUDA IPC handle open/close helpers (`open_cuda_ipc`, `close_cuda_ipc`)
**Location**: `src/service.rs:65-99`

**Assessment**: These are implementation details of how the server processes the CUDA IPC handles described in FR-011. They don't represent a separate feature — they're the mechanism by which the server fulfills FR-002, FR-003, and the updated FR-011. No new spec is needed; updating FR-011 (Proposal 2) fully covers this code.

**Confidence**: HIGH

**Action**:
- [ ] Approve (no new spec needed)
- [ ] Reject (create separate spec)
- [ ] Modify
