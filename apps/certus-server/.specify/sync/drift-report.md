# Spec Drift Report

Generated: 2026-05-29
Project: certus-server

## Summary

| Category | Count |
|----------|-------|
| Specs Analyzed | 1 |
| Requirements Checked | 17 |
| Aligned | 15 (88%) |
| Drifted | 2 (12%) |
| Not Implemented | 0 (0%) |
| Unspecced Code | 3 |

## Detailed Findings

### Spec: 001-grpc-dispatcher-server - gRPC Dispatcher Server

#### Aligned

- FR-001: gRPC service exposes lookup, check, remove, populate, touch, clear_memory_tier -> `proto/dispatcher.proto`, `src/service.rs`
- FR-002: Populate accepts list of (key, ipc_handle), returns per-entry results -> `src/service.rs`
- FR-003: Lookup internally calls `batch_lookup()` for parallel cold promotion; IPC handles deduplicated within batch -> `src/service.rs` (updated 2026-05-28)
- FR-004: Check accepts list of keys, returns list of boolean results -> `src/service.rs`
- FR-005: Remove accepts list of keys, returns per-entry results -> `src/service.rs`
- FR-005b: Touch accepts list of keys, returns per-entry results -> `src/service.rs`
- FR-006: CLI accepts device PCI(s), listen address/port, TLS cert/key paths -> `src/main.rs`
- FR-007: Per-entry results include key, success, error_code, error_message -> `proto/dispatcher.proto`, `src/service.rs`
- FR-008: Server auto-initializes dispatcher on startup using CLI PCI addresses -> `src/main.rs`
- FR-008b: Dispatch map starts fresh each launch -> confirmed
- FR-008c: Memory-tier pool registered with CUDA via cudaHostRegister -> confirmed
- FR-010: Python test client exercises all gRPC methods with batch operations -> `python-client/`
- FR-013: Multiple connections supported, processing serialized via Mutex -> `src/service.rs`
- FR-014: Optional TLS via --tls-cert/--tls-key -> `src/main.rs`
- FR-015: Pre-validates batch for duplicate keys, rejects entire batch -> `src/service.rs`
- FR-016: Multiple --device-pci arguments supported -> `src/main.rs`
- FR-017: ClearMemoryTier gRPC method exposed -> `src/service.rs`

#### Drifted

- **FR-009**: Spec says "shut down dispatcher when receiving SIGTERM/SIGINT" but code only handles SIGINT via
  `tokio::signal::ctrl_c()`. SIGTERM is not caught.
  - Location: `src/main.rs`
  - Severity: moderate

- **FR-011 / Key Entities / IpcHandle**: Spec says IpcHandle contains "a 64-byte CUDA IPC memory handle and a
  size (uint32)". The proto now has a third field `gpu_device_id` (int32, field 3). The spec definition in
  FR-011 and the Key Entities section does not mention this field.
  - Location: `proto/dispatcher.proto:33`, `src/service.rs:209,305`
  - Severity: moderate — the field is required for correct multi-GPU operation; missing from spec entirely

#### Not Implemented

(none)

### Unspecced Code

| # | Feature | Location | Severity | Notes |
|---|---------|----------|----------|-------|
| 1 | **Global persistent IPC handle cache** (`IpcCacheEntry` with `dev_ptr`, `gpu_device_id`, `refcount`; keyed by 64-byte handle bytes; lives on `DispatcherService` for the lifetime of the server process) | `src/service.rs:27-115` | high | Previous report only noted within-batch deduplication (FR-003). The current cache is a persistent cross-request structure that eliminates repeated open/close across concurrent batches and removes serialization on CUDA's global IPC lock. Qualitatively different from within-batch reuse — requires a new requirement. |
| 2 | **`cudaSetDevice(gpu_device_id)` before `cudaIpcOpenMemHandle`** | `src/service.rs:65-75` | high | When a handle is not yet in the cache, the server calls `cudaSetDevice(gpu_device_id)` (when `gpu_device_id >= 0`) prior to opening the IPC handle. This behavior is entirely absent from the spec. It is necessary for correct operation on multi-GPU systems and to avoid "resource already mapped" errors when the server CUDA context is not on the allocating device. |
| 3 | **Reference-counted cache eviction** (`refcount` incremented on open, decremented on close; `cudaIpcCloseMemHandle` called only when refcount reaches zero) | `src/service.rs:106-115` | moderate | The mechanism by which cached handles are eventually closed — reference counting rather than LRU or explicit eviction — is not specified anywhere. |

## Recommendations

1. **Add FR-018** (new): Specify the global IPC handle cache that persists across requests. Include the
   keying strategy (64-byte handle bytes), the fields stored (`dev_ptr`, `gpu_device_id`, `refcount`),
   and the motivation (eliminate "resource already mapped" errors, remove CUDA IPC lock serialization).

2. **Add FR-019** (new): Specify that the server calls `cudaSetDevice(gpu_device_id)` before opening an
   IPC handle that is not already cached, when `gpu_device_id >= 0`.

3. **Update FR-011 and Key Entities IpcHandle**: Add `gpu_device_id` (int32, field 3) to the IpcHandle
   description. Explain that the client provides the CUDA device ordinal that allocated the memory so the
   server can set the correct device before opening the handle.

4. **Fix SIGTERM handling (FR-009)**: Add SIGTERM handler alongside the existing SIGINT handler
   (`tokio::signal::ctrl_c`).
