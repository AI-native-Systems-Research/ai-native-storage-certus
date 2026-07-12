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
runs **separately** on the host (it owns SPDK/NVMe/hugepages). The container
offloads to it over gRPC.

### What the container does and does NOT provide

A container image bundles **userspace** dependencies (vLLM, torch, the CUDA
*runtime*, this connector) — those are baked in and reproducible. It cannot
bundle **kernel/driver** dependencies:

- **GPU driver** (`libcuda.so`) — injected from the host at run time by the
  NVIDIA container runtime; it must match the host kernel module.
- **SPDK / hugepages / vfio** — needed by the *server*, which runs on the host.

So running the benchmark is always "this image **plus** a provisioned host." The
image *declares* the GPU need (see the `org.certus.gpu-required` label); a
host-level component *fulfills* it. Two helper scripts wire this up.

### Running the benchmark end-to-end

```bash
# 0. Build the image (context = repo root; needs certus-grpc-connector/ + dataset).
#    This host keeps the image on /mnt/certus1, so pass the store paths:
podman --root /mnt/certus1/podman/storage --runroot /mnt/certus1/podman/run \
    build -f certus-grpc-connector/Dockerfile -t certus-grpc-bench .

# 1. Host prerequisites — ONE-TIME, root. Installs nvidia-container-toolkit +
#    CDI spec (GPU), AND binds the server NVMe drives to vfio-pci + allocates
#    1G hugepages for the DRAM tier. Defaults to the 0000:61-64 (NUMA-0) drive
#    set; size the tier to available RAM with HUGEPAGES_1G:
sudo HUGEPAGES_1G=48 ./certus-grpc-connector/setup-host.sh

# 2. Build + launch certus-server on the host (drives already bound in step 1).
#    --memory-tier-size must fit the hugepages allocated above.
deps/build_spdk.sh && cargo build --release -p certus-server
target/release/certus-server --device-pci 0000:61:00.0 --device-pci 0000:62:00.0 \
    --device-pci 0000:63:00.0 --device-pci 0000:64:00.0 \
    --memory-tier-size 44G --listen 0.0.0.0:50051 --format

# 3. Run the workload container against the server.
GPU=0 CERTUS_SERVER=localhost:50051 ./certus-grpc-connector/run-bench.sh
```

`setup-host.sh` targets the server's own drive set (0000:61-64), leaving the
c1-c4 drives (filesystems / the podman image store) untouched. `run-bench.sh`
preflights the CDI spec and image, then launches `podman run`
with `--device nvidia.com/gpu=$GPU`, `--ipc=host` (so the host server can open
the container's CUDA IPC handles), the HF cache mount, and `CERTUS_SERVER`. If
the GPU prerequisite is missing it prints the exact `setup-host.sh` command
rather than failing with a cryptic `libcuda.so` error.

The container entrypoint waits for `CERTUS_SERVER` to accept connections
(failing fast if it never comes up), then runs the 450-conv / 12-turn workload.

**Overridable env** (`-e` on `podman run`, or exported before `run-bench.sh`):
`GPU`, `CERTUS_SERVER`, `NUM_CONVS`, `MODEL`, `SLAB_SIZE_BYTES`, `DATASET_PATH`,
`HF_TOKEN`, `HF_CACHE`, and `PODMAN_STORE` / `PODMAN_RUNROOT` for the store paths.

### Manual run (without the wrapper)

```bash
podman --root /mnt/certus1/podman/storage --runroot /mnt/certus1/podman/run \
    run --rm --device nvidia.com/gpu=0 --ipc=host \
    -e CERTUS_SERVER=<host>:50051 \
    -e HF_TOKEN=<token> \
    -v $HOME/.cache/huggingface:/root/.cache/huggingface \
    certus-grpc-bench
```

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
