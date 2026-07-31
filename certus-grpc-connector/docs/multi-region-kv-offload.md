<!-- SPDX-License-Identifier: Apache-2.0 -->
# Multi-region KV offload — completing a real 0.23+ run

**Status:** implemented. This lands the multi-region path described below and was
validated end-to-end on vLLM 0.24 (Llama-3-8B, 32 layers). The connector no longer
guards on `len(tensors) != 1`; it fans out to one IPC handle per layer.

## The problem

Through vLLM 0.20/0.22 a KV block is presented to the offloading worker as **one
coalesced tensor** spanning all layers (`len(kv_caches.tensors) == 1`, per-block
stride = the full block, 2 MiB for Llama-3-8B). The connector's model matches that
exactly: it exports **one** CUDA IPC handle for the whole KV-cache allocation and
addresses each block by byte offset.

```python
# certus_grpc_connector/handler.py
def _ipc_handle(kv, block_id, size):
    return pb.IpcHandle(
        cuda_ipc_handle=kv.handle_bytes,
        size=size,
        gpu_device_id=kv.gpu_device_id,
        offset=kv.block_offset(block_id),   # base + block_id * block_bytes
    )
```

`CopyToStoreEntry` and `LookupEntry` in `apps/certus-server/proto/dispatcher.proto`
each carry exactly **one** `IpcHandle` per key.

At vLLM **0.23** the KV cache split into **per-layer tensors**
(`len(tensors) == N`, N = model KV-cache layer count — 32 for Llama-3-8B), each a
**separate allocation** shaped `(num_blocks, page_size_bytes)` int8. One logical
block is now N discontiguous regions. Reading `tensors[0]` silently offloads
**layer 0 only** (65 536 of 2 097 152 bytes) — the failure mode this change fixes.
`compat.extract_gpu_ptrs` now returns one `(ptr, stride)` per layer tensor, and the
whole store/load path carries N handles per key, so 0.23/0.24/0.26 complete a real
full-block run.

## The design (server-side N-DMA, no connector buffering)

One KV block = **N regions**, one per layer. The offload path fans one logical
store/load into N DMAs; **the index and on-disk storage stay 1:1 per key.**

### Invariants (do not break these)

- **Index & storage stay 1:1 per key.** Each block is still one contiguous slot,
  one dispatch-map entry. No index growth. This preserves the colocation wins:
  sequential 2 MiB SSD I/O, **atomic single-unit eviction** (a block is present or
  absent, never half its layers), one contiguous readback.
- **No staging buffer in the connector.** The N per-layer DMAs line up at the
  destination themselves — gather-on-write, scatter-on-read. The connector does not
  coalesce; the fragmentation is inherent to the model's KV layout and exists only
  at the GPU source, at DMA time.

### Mechanics

The server lays the N regions out **colocated** inside the single slot, layer L at
`slot_base + L * page_size`:

```
store:  slot + L*page  ←  tensor_L + block_id*page     for L in 0..N
load:   tensor_L + block_id*page  ←  slot + L*page      for L in 0..N
```

N `cudaMemcpyAsync` per block, landing/originating in one contiguous slot. The
server stays layer-agnostic — it copies N regions of `page_size` each into/out of a
slot of `N * page_size`; it does not need to know these are "layers."

### Carrying the N-ness

The N per-layer allocations are **fixed for the engine's lifetime**. Two proto
encodings were considered:

1. **Register-once.** A new registration RPC carries the N base `IpcHandle`s (and
   their `page_size`); `CopyToStoreEntry`/`LookupEntry` gain a `block_id` and drop
   the per-key handle. Avoids resending ~N×80 B of handle bytes on every key — a
   later optimization.
2. **Repeated handle (implemented).** `CopyToStoreEntry`/`LookupEntry` carry
   `repeated IpcHandle ipc_handles = 3` (each with its own `offset = block_id*page`).
   No new RPC; handles resent each call. Chosen for the first working build; the
   singular field 2 is retained so `populate` and single-tensor 0.20/0.22 (N=1) keep
   working with no version branch.

The field is **`repeated`** — inherently variable-length, so **no schema limit is
needed**. N is bounded by the layer count; the server sanity-checks that the summed
region bytes match the reserved slot size.

## Scope

This is a `dispatcher.proto` + certus-server + connector change. The server stays
version-independent — it copies N regions of `page_size` into/out of one
`N * page_size` slot without knowing they are layers. `dispatcher-p2p` explicitly
rejects N>1 (it has no DRAM-staging scatter path) and is out of the E2E path.

See also: the connector's `compat.py` capability matrix, and the
`certus-grpc-multiversion-compat` / `certus-grpc-multiregion-design` project notes.
