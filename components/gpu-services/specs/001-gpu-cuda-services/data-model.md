# Data Model: GPU CUDA Services

**Date**: 2026-04-29
**Feature**: GPU CUDA Services (001)

## Entities

### GpuDevice

Represents a discovered NVIDIA GPU meeting minimum compute
requirements.

| Field | Type | Description |
|-------|------|-------------|
| device_index | u32 | CUDA device ordinal |
| name | String | GPU model name (e.g., "NVIDIA A100") |
| memory_bytes | u64 | Total global memory in bytes |
| compute_major | u32 | Compute capability major version |
| compute_minor | u32 | Compute capability minor version |
| pci_bus_id | String | PCI BDF address string |

**Constraints**:
- `compute_major >= 7` (Volta+, enforced at discovery)
- `memory_bytes > 0`
- `device_index` unique within a single scan

**Lifecycle**: Created during `initialize()`, immutable after creation,
discarded on `shutdown()`.

### CudaIpcHandle (internal)

Deserialized from base64 wire format into native CUDA type.

| Field | Type | Description |
|-------|------|-------------|
| raw_handle | [u8; 64] | cudaIpcMemHandle_t bytes |
| size | u64 | Buffer size in bytes |

**Constraints**:
- `size > 0`
- `raw_handle` must correspond to a valid, non-closed IPC export

**Lifecycle**: Created by deserialization, consumed by
`open_ipc_handle` to obtain a device pointer, then no longer needed.

### IpcHandle (existing in interfaces crate)

Already defined in `components/interfaces/src/idispatcher.rs`:

| Field | Type | Description |
|-------|------|-------------|
| address | *mut u8 | GPU memory base address |
| size | u32 | Buffer size in bytes |

**Note**: The gpu-services component will use this existing type as
the output of IPC handle opening. The `address` field holds the device
pointer obtained from `cudaIpcOpenMemHandle`.

### DmaBuffer (existing in interfaces crate)

Already defined in `components/interfaces/src/spdk_types.rs`. GPU
services creates instances via `DmaBuffer::from_raw()` with:
- `ptr`: GPU device pointer from opened IPC handle
- `len`: Buffer size from deserialized handle
- `free_fn`: Wrapper around `cudaIpcCloseMemHandle`
- `numa_node`: NUMA node of the GPU's PCIe slot

### PinnedRegion (internal tracking)

Tracks GPU memory regions that have been verified/pinned.

| Field | Type | Description |
|-------|------|-------------|
| device_ptr | *mut u8 | GPU device memory pointer |
| size | u64 | Region size in bytes |
| device_index | u32 | Owning GPU device index |
| is_verified | bool | Contiguity/pin check passed |

**Constraints**:
- `device_ptr` must be non-null
- `size > 0`
- `is_verified` must be true before DMA buffer creation

**Lifecycle**: Created when an IPC handle is opened, marked verified
after passing checks, released when the corresponding DMA buffer is
dropped or on explicit unpin.

## State Transitions

### Component Lifecycle

```
Uninitialized -> Initialized -> ShutDown
     |                              |
     +------------------------------+
              (re-initialize)
```

- **Uninitialized**: No CUDA context, no devices discovered
- **Initialized**: CUDA loaded, qualifying devices enumerated, ready
  for operations
- **ShutDown**: All IPC handles closed, CUDA context released

### IPC Handle Flow

```
base64 payload -> Deserialized -> Opened (device ptr) -> Verified -> DmaBuffer
                                       |                                 |
                                       +--- Close (on error) <-----------+
                                                                    (on drop)
```

## Relationships

```
GpuDevice 1───* PinnedRegion
PinnedRegion 1───1 DmaBuffer (optional, if created)
CudaIpcHandle ───> PinnedRegion (via cudaIpcOpenMemHandle)
```

## Wire Format (Python → Rust via Unix Domain Socket)

```
┌──────────────────────────────────────────────┐
│ 4 bytes: payload length (LE u32)             │
├──────────────────────────────────────────────┤
│ N bytes: base64-encoded string               │
│   Decodes to 72 bytes:                       │
│   ├── bytes[0..64]:  cudaIpcMemHandle_t      │
│   └── bytes[64..72]: buffer size (LE u64)    │
└──────────────────────────────────────────────┤
Response: 1 byte (0x01 = ACK, 0x00 = NACK)
```
