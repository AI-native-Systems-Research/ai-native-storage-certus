# certus-grpc-connector

vLLM **OffloadingSpec** plugin for the Certus storage system, talking to a
running **`certus-server`** over gRPC. Implements vLLM's `OffloadingSpec` ABC so
that `OffloadingConnectorScheduler` can offload KV cache blocks to tiered
DRAM + raw NVMe storage — with the storage engine in a separate process.

This is the gRPC-client counterpart to the in-process [`certus-connector`](../certus-connector).
Same vLLM plugin contract; the difference is where the engine lives:

| | `certus-connector` (in-process) | `certus-grpc-connector` (this package) |
|---|---|---|
| Engine | embedded PyO3 Rust (`certus_native`) | remote `certus-server` |
| Transport | raw device pointers, in-process | CUDA IPC handles over gRPC |
| SPDK/CUDA init | inside the vLLM process | owned by the server |
| Language | Rust + Python | pure Python |

## How it fits into vLLM

```
vLLM OffloadingConnectorScheduler          ← vLLM's internal scheduler
  │  loads via kv_connector_extra_config:
  │    spec_module_path = "certus_grpc_connector.spec"
  │    spec_name = "CertusGrpcOffloadingSpec"
  ▼
CertusGrpcOffloadingSpec (OffloadingSpec)  ← OUR plugin entry point
  │  opens ONE shared grpc channel/stub to certus-server:
  ├─ get_manager()  → GrpcCertusOffloadingManager  (index/alloc/eviction RPCs)
  └─ get_handlers() → GpuToCertusHandler / CertusToGpuHandler  (DMA RPCs)
                                  │
                                  ▼
                          certus-server (Dispatcher gRPC service)
```

## RPC mapping

| vLLM manager/handler call | gRPC RPC |
|---|---|
| `lookup(key)` | `Check(keys)` |
| `prepare_store(keys)` | `Check` (filter existing) + `Reserve(keys, sizes)` |
| store `transfer_async` | `CopyToStore(keys, ipc_handles)` |
| `complete_store(success)` | `CommitStore(keys)` / `AbortStore(keys)` |
| `prepare_load(keys)` | `Pin(keys, promote=true)` |
| load `transfer_async` | `Lookup(keys, ipc_handles)` |
| `complete_load(keys)` | `Unpin(keys)` |
| `touch(keys)` | `Touch(keys)` |
| `take_events()` | `TakeEvents(max_events=0)` |

## GPU addressing: the `offset` field

vLLM's KV cache is **one large device allocation**; block *i* is a byte offset
into it. A CUDA IPC handle, however, always resolves on the server side to the
**base of the containing allocation** — the offset is not carried by the handle.

To bridge this, the proto `IpcHandle` carries a `uint64 offset` field. The
connector takes **one** IPC handle on the KV-cache allocation and sends, per
block, `offset = (data_ptr - alloc_base) + block_id * stride`; the server DMAs
at `cudaIpcOpenMemHandle(handle) + offset`. The offset defaults to 0, so
existing per-block-allocation clients are unaffected.

Offset math lives in `gpu.py` (`KvCacheIpc.block_offset`). The allocation base
is found with `cuMemGetAddressRange`, since the handle resolves to the
allocation base, not `tensor.data_ptr()`.

> **Known risk:** some PyTorch caching-allocator pointers are not valid
> `cudaIpcGetMemHandle` inputs. If the KV-cache allocation is not IPC-exportable,
> `gpu.py` raises; the intended fallback is a connector-owned bounce-buffer pool
> confined to `gpu.py`, leaving the manager/handler contract unchanged.

## Package contents

| Path | What |
|------|------|
| `certus_grpc_connector/spec.py` | `CertusGrpcOffloadingSpec` — plugin entry point, shared channel |
| `certus_grpc_connector/manager.py` | `GrpcCertusOffloadingManager` — maps manager calls to RPCs |
| `certus_grpc_connector/handler.py` | Store/load handlers (async gRPC transfers) |
| `certus_grpc_connector/mediums.py` | `CertusLoadStoreSpec` + `BlockLocation` |
| `certus_grpc_connector/gpu.py` | CUDA IPC handle + per-block offset math |
| `certus_grpc_connector/client.py` | Channel/stub factory |
| `certus_grpc_connector/dispatcher_pb2*.py` | Generated gRPC stubs (checked in) |

## Build & test

```bash
# Regenerate gRPC stubs from the server proto (after proto changes)
bash generate_pb.sh

# Unit tests — no server, GPU, or vLLM required (vLLM is stubbed in conftest.py)
python3 -m pytest tests/ -v

# Install into a vLLM environment
pip install -e .
```

## Docker (workload driver)

`Dockerfile` builds a **client-side** image only — vLLM + this connector + the
multi-turn workload driver (`run_multiturn_grpc_certus.py`). The `certus-server`
runs **separately** on the host (it owns SPDK/NVMe/hugepages; build it there
with `deps/build_spdk.sh` + `cargo build -p certus-server`). The container
offloads to it over gRPC.

```bash
# Build (context = repo root; needs certus-grpc-connector/ + the dataset)
podman build -f certus-grpc-connector/Dockerfile -t certus-grpc-bench .

# Run against a server reachable at CERTUS_SERVER. --ipc=host lets the server
# open the CUDA IPC handles this container's vLLM process exports.
podman run --rm --gpus all --ipc=host \
    -e CERTUS_SERVER=<host>:50051 \
    -e HF_TOKEN=<token> \
    -v $HOME/.cache/huggingface:/root/.cache/huggingface \
    certus-grpc-bench
```

The entrypoint waits for `CERTUS_SERVER` to accept connections (failing fast if
it never comes up), then runs the 450-conv / 12-turn workload. Override
`NUM_CONVS`, `MODEL`, `SLAB_SIZE_BYTES`, `DATASET_PATH` via `-e`.

## vLLM configuration

```json
{
    "spec_name": "CertusGrpcOffloadingSpec",
    "spec_module_path": "certus_grpc_connector.spec",
    "server": "localhost:50051",
    "slab_size_bytes": 131072
}
```

## Semantics preserved from the in-process path

- **Eviction only at `prepare_store`** — `Reserve` does server-side LRU eviction;
  a per-key `ALLOCATION_FAILED` maps to `prepare_store` → `None` (hard reject),
  matching vLLM's worker which asserts store success.
- **Pin bracket** — `prepare_load` pins (protect from eviction), `complete_load`
  unpins; blocks between them cannot be evicted.
- **Split-phase store** — `Reserve` (invisible slot) → `CopyToStore` (GPU→DRAM
  DMA) → `CommitStore` (visible + SSD write-through).
- **Events are lossy** — `TakeEvents` reports `dropped_count`; only `REMOVED`
  (not `DEMOTED`, which stays loadable from SSD) is surfaced as an eviction.
