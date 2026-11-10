# Data Model: GPU-to-SSD DMA Buffer Preparation

## Entities

### CUDA IPC Handle Payload (input)

| Field | Type | Description |
|-------|------|-------------|
| handle_bytes | [u8; 64] | Raw `cudaIpcMemHandle_t` content |
| size | u64 (LE) | Buffer size in bytes |

Wire format: 72 bytes total, base64-encoded for gRPC transport.

### DmaBuffer (output)

Uses existing `interfaces::spdk_types::DmaBuffer` with:

| Field | Source | Value |
|-------|--------|-------|
| ptr | `cudaIpcOpenMemHandle` result | GPU device pointer |
| len | Decoded from payload | Buffer size in bytes |
| free_fn | Selected at creation time | `cuda_ipc_close_only` OR `cuda_ipc_unpin_and_close` |
| numa_node | Constant | -1 (GPU memory has no CPU NUMA affinity) |
| metadata | Set by implementation | `{"source": "gpu_ipc", "device": "<idx>"}` |

### Pin State (internal decision)

| State | free_fn selected | Drop behavior |
|-------|-----------------|---------------|
| Was NOT pinned (we pinned it) | `cuda_ipc_unpin_and_close` | Unpin → Close IPC handle |
| Was already pinned | `cuda_ipc_close_only` | Close IPC handle only |

## State Transitions

```
Input: base64 &str + Option<u32> device_index
  │
  ├─ [Optional] cudaSetDevice(device_index)
  │
  ├─ decode_ipc_payload(base64) → (handle_bytes, size)
  │
  ├─ cudaIpcOpenMemHandle(handle_bytes, LAZY_PEER_ACCESS) → dev_ptr
  │
  ├─ Query component state: is dev_ptr in `pinned` set?
  │     ├─ YES → was_already_pinned = true
  │     └─ NO  → pin_memory(dev_ptr); was_already_pinned = false; LOG
  │
  ├─ Select free_fn based on was_already_pinned
  │
  ├─ DmaBuffer::from_raw(dev_ptr, size, free_fn, -1)
  │
  └─ Return Ok(DmaBuffer)
```

## Error States

| Error condition | Cleanup required | Error message |
|-----------------|-----------------|---------------|
| Not initialized | None | "Not initialized: call initialize() first" |
| Invalid base64 | None | "Invalid base64: {detail}" |
| Wrong payload size | None | "Payload must be exactly 72 bytes, got {n}" |
| Zero-size buffer | None | "Buffer size must be > 0" |
| cudaSetDevice fails | None | "cudaSetDevice({idx}) failed: {err}" |
| cudaIpcOpenMemHandle fails | None | "cudaIpcOpenMemHandle failed: {err}" |
| Pin fails | Close IPC handle | "Failed to pin memory: {err}" |
| DmaBuffer::from_raw fails | Unpin (if pinned) + Close IPC | "DmaBuffer creation failed: {err}" |

## Relationships

- `prepare_memory_for_spdk` reuses `ipc::decode_ipc_payload` and `ipc::open_ipc_handle`
- Pin-state tracking uses `GpuState.pinned: HashSet<usize>`
- The returned `DmaBuffer` is passed directly to SPDK NVMe read/write operations
- Free functions use `cuda_ffi::cudaIpcCloseMemHandle` and `cuda_ffi::cudaHostUnregister`
