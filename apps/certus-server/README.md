# certus-server

A gRPC server that exposes the Certus IDispatcher interface to remote clients, enabling GPU-accelerated cache operations over CUDA IPC.

## Overview

certus-server assembles the full Certus component stack (SPDK block devices, extent managers, dispatch map, GPU services, and dispatcher) and serves it as a gRPC endpoint. Clients allocate GPU memory, share it via CUDA IPC handles, and the server performs DMA transfers between GPU memory and NVMe storage.

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│  Python Client (or any gRPC client)                             │
│  - Allocates GPU memory (PyTorch / CUDA)                        │
│  - Obtains cudaIpcMemHandle (64 bytes)                          │
│  - Sends batch requests over gRPC                               │
└───────────────────────────┬─────────────────────────────────────┘
                            │ gRPC (protobuf)
                            ▼
┌─────────────────────────────────────────────────────────────────┐
│  certus-server (Rust / tonic)                                   │
│                                                                 │
│  ┌───────────────────────────────────────────────────────────┐  │
│  │ gRPC Service Layer (service.rs)                           │  │
│  │ - Opens CUDA IPC handles in server's GPU context          │  │
│  │ - Validates requests, rejects duplicate keys              │  │
│  │ - Maps per-entry results back to client                   │  │
│  └───────────────────────────┬───────────────────────────────┘  │
│                              ▼                                   │
│  ┌───────────────────────────────────────────────────────────┐  │
│  │ IDispatcher                                               │  │
│  │ - populate: GPU → host DMA → staging → NVMe              │  │
│  │ - lookup:   NVMe → staging → host → GPU DMA              │  │
│  │ - check:    key existence query                           │  │
│  │ - remove:   evict entry, free storage                     │  │
│  └──────┬────────────────┬───────────────────┬──────────────┘  │
│         ▼                ▼                   ▼                  │
│  ┌────────────┐  ┌──────────────┐  ┌─────────────────────┐     │
│  │IGpuServices│  │IDispatchMap  │  │IExtentManager       │     │
│  │(CUDA DMA)  │  │(key→location)│  │(block allocation)   │     │
│  └────────────┘  └──────────────┘  └─────────┬───────────┘     │
│                                               ▼                 │
│                                    ┌─────────────────────┐      │
│                                    │IBlockDevice (SPDK)  │      │
│                                    │  metadata + data    │      │
│                                    └─────────────────────┘      │
└─────────────────────────────────────────────────────────────────┘
```

### Component Stack

The server initializes components in order:

1. **SPDK Environment** — userspace NVMe driver framework
2. **GPU Services** — CUDA memory operations and DMA
3. **Block Device (metadata)** — NVMe device for dispatch-map persistence
4. **Extent Manager** — fixed-size block allocation over the metadata device
5. **Dispatch Map** — maps cache keys to storage locations with reference counting
6. **Dispatcher** — orchestrates GPU↔NVMe data movement through the above

## gRPC API

Defined in `proto/dispatcher.proto` (package `certus.dispatcher.v1`):

| RPC | Request | Description |
|-----|---------|-------------|
| `Populate` | `BatchPopulateRequest` | DMA-copy data from client GPU memory into the cache |
| `Lookup` | `BatchLookupRequest` | DMA-copy cached data back to client GPU memory |
| `Check` | `BatchCheckRequest` | Check whether keys exist (no data transfer) |
| `Remove` | `BatchRemoveRequest` | Evict entries and free storage |

All operations are **batch-capable** — clients submit lists of entries in a single RPC to avoid per-key round-trip overhead. The server iterates entries internally, calling the singular IDispatcher methods for each.

### CUDA IPC Protocol

Clients share GPU memory with the server using CUDA IPC:

1. Client allocates GPU memory (e.g., via PyTorch `torch.zeros(..., device="cuda")`)
2. Client calls `cudaIpcGetMemHandle` to obtain a 64-byte opaque handle
3. Client sends the handle bytes in the `IpcHandle.cuda_ipc_handle` field
4. Server calls `cudaIpcOpenMemHandle` to map the memory into its CUDA context
5. Server performs DMA operations on the mapped pointer
6. Server calls `cudaIpcCloseMemHandle` when done

## Building

```bash
# Requires SPDK pre-built at deps/spdk-build/
cargo build -p certus-server
```

The build script automatically downloads `protoc` if not found on the system.

## Running

```bash
certus-server --device-pci 0000:d9:00.0
```

### CLI Options

| Flag | Required | Default | Description |
|------|----------|---------|-------------|
| `--device-pci` | Yes | — | PCI address(es) of NVMe device(s) (DDDD:BB:DD.F); repeatable |
| `--listen` | No | `0.0.0.0:50051` | gRPC listen address |
| `--tls-cert` | No | — | Path to TLS certificate (PEM) |
| `--tls-key` | No | — | Path to TLS private key (PEM) |

### Prerequisites

- Linux with VFIO/IOMMU enabled
- NVMe devices bound to VFIO (not kernel nvme driver)
- Hugepages configured (SPDK requirement)
- NVIDIA GPU with CUDA IPC support
- `memlock` ulimit set to unlimited

## Python Test Client

Located in `python-client/`:

```bash
# Install dependencies
pip install -r python-client/requirements.txt
pip install torch

# Regenerate protobuf stubs (if proto changes)
bash python-client/generate_pb.sh

# Run tests
python3 python-client/test_client.py --server localhost:50051
```

The test client exercises:
- Batch populate (10 entries)
- Batch check (existence verification)
- Batch lookup (data retrieval)
- Batch remove (eviction)
- Duplicate key rejection
- Non-existent key error handling
- Large batch operations (1000 entries)

### Test Client Requirements

- NVIDIA GPU with CUDA
- PyTorch with CUDA support (`torch.cuda.is_available()`)
- `libcudart.so` accessible via `LD_LIBRARY_PATH`
