# KV Cache I/O Access Patterns

Summary of access patterns Certus must support, derived from vLLM (v0.12+) and llm-d-kv-cache source code.

---

## Transfer Granularity: Old vs New Layout

The cross-layer layout (PR #27743, vLLM 0.12.0) consolidated per-layer fragments into one contiguous block.

| Model | Old (per-layer, K or V) | New (cross-layer, all layers) |
|-------|------------------------|-------------------------------|
| Llama-3.1-8B (TP=1) | 32 KB | 2.0 MB |
| Llama-3.1-70B (TP=1) | 32 KB | 5.0 MB |
| Llama-3.1-70B (TP=8) | 8 KB | 0.625 MB |
| DeepSeek-V3 MLA (TP=1) | 18 KB | 2.14 MB |
| Qwen2.5-7B (TP=1) | 16 KB | 0.88 MB |
| Gemma-2-27B (TP=1) | 64 KB | 5.75 MB |

**Formula (new):** `gpu_page = 2 × block_size(16) × num_kv_heads × head_dim × dtype_bytes × num_layers`

**Formula (old):** `half_page = block_size(16) × num_kv_heads × head_dim × dtype_bytes` (one layer, K or V)

---

## Offloaded Block = Storage I/O Unit

One offloaded block = `block_size_factor` GPU blocks bundled together for storage.

| Parameter | Default | Configurable |
|-----------|---------|--------------|
| GPU block size | 16 tokens | Yes (`block_size` in vLLM config) |
| block_size_factor | **1** | Yes (set `block_size` in `kv_connector_extra_config`) |
| Offloaded block size | 16 tokens (= 1 GPU block) | e.g. 256 tokens if factor=16 |
| File size (llm-d) | gpu_page × block_size_factor | Scales with factor |

**Note**: `block_size_factor` (number of GPU blocks bundled into one offloaded block / one storage file) defaults to 1. The value 16 (= 256 tokens = 16 GPU blocks per file) is a deployment choice, not a built-in default. Examples below show both:

| Model | File size (factor=16) | File size (factor=1) |
|-------|----------------------|---------------------|
| Llama-8B TP=1 | 32 MB | 2 MB |
| Llama-70B TP=1 | 80 MB | 5 MB |
| Llama-70B TP=8 | 10 MB | 0.625 MB |
| Llama-8B TP=8 | 4 MB | 0.25 MB |

---

## Write Pattern (GPU → Storage)

| Property | Description |
|----------|-------------|
| Trigger | After each engine step, deferred to next step start |
| Granularity | One offloaded block (256 tokens, all layers) per write |
| Ordering | Sequential within a request (monotonically increasing block index) |
| Concurrency | Multiple requests offloading simultaneously |
| Semantics | Write-once / content-addressed (same hash = same data) |
| Dedup | Skip write if block already exists (checked by hash) |
| Atomicity | Write to temp file + rename (llm-d fs backend) |

### Write data flow (vLLM CPU offload — no persistent storage)

```
GPU blocks (block_size_factor blocks per offloaded group)
  → cuMemcpyBatchAsync (factor entries × ~2 MB each; 1 entry if factor=1)
  → CPU pinned tensor rows (one row per GPU block, stride-based addressing)
```

Data stays in CPU pinned memory (no disk write). This is a pure GPU↔CPU swap.

### Write data flow (llm-d fs_backend — persistent storage)

```
GPU blocks → CPU buffer (same DMA as above)
  → file write: temp file + atomic rename (one file per content-hash per offloaded block)
```

Note: CPU-side rows use `tensor.stride(0)` for addressing — contiguous with standard pinned allocation, potentially strided with mmap-backed SharedOffloadRegion.

---

## Read Pattern (Storage → GPU)

| Property | Description |
|----------|-------------|
| Trigger | Scheduler detects prefix hit in offloaded cache |
| Granularity | One offloaded block per storage read |
| Access pattern | Prefix-sequential (always longest contiguous prefix from a start offset) |
| Concurrency | Up to ~48 read-preferring workers (64 threads × 0.75 ratio; can also serve writes) |
| Fan-out | N independent reads in parallel (one per offloaded block / file) |
| Latency sensitivity | Blocks token generation start; on critical path |

### Read data flow (vLLM CPU offload — no storage I/O)

```
CPU pinned tensor rows (data already in memory from prior GPU→CPU swap)
  → cuMemcpyBatchAsync (factor entries × ~2 MB each)
  → GPU blocks (scattered destination block IDs)
```

### Read data flow (llm-d fs_backend)

```
Storage (N parallel file reads via thread pool, one file per offloaded block)
  → Thread-local read buffer (contiguous per file)
  → cudaMemcpyAsync loop (1 call per block for cross-layer; num_layers calls for per-layer)
  → GPU KV cache tensor (contiguous cross-layer block)
```

---

## DMA Transfer Patterns (GPU ↔ CPU Staging)

### vLLM CPU Offloading (`gpu_worker.py` → `swap_blocks_batch`)

| Property | Value |
|----------|-------|
| API | `cuMemcpyBatchAsync` (CUDA 12.8+), fallback loop of `cudaMemcpyAsync` |
| Entries per batch | block_size_factor × num_groups × blocks_in_job |
| Per-entry size | `data_ref.page_size_bytes` = gpu_page_size (~0.5–5 MB) |
| Submission | Single driver call for all entries |
| Source (load) | CPU pinned tensor rows (contiguous with standard pinned; stride-based with mmap regions) |
| Destination (load) | Scattered GPU block indices (non-contiguous) |
| Ordering | Each job's CUDA stream waits on the previous job's end_event |

### llm-d fs_backend (`tensor_copier.cu` → `copy_blocks_via_cuda_memcpy`)

| Property | Cross-layer layout (new) | Per-layer layout (old) |
|----------|-------------------------|----------------------|
| API | `cudaMemcpyAsync` in loop | `cudaMemcpyAsync` in loop |
| Calls per GPU block | 1 (single tensor covers all layers) | num_layers (one call per layer tensor) |
| Per-call size | `m_tensor_block_size` (~2 MB, all layers packed) | `m_tensor_block_size` (~32 KB, one layer) |
| Calls per offloaded block (factor=16) | 16 × 1 = 16 | 16 × num_layers |
| Calls per offloaded block (factor=1) | 1 | num_layers |
| Source (load) | Contiguous read buffer (one file read into memory) | Same |
| Destination (load) | Single GPU tensor (cross-layer) | Per-layer GPU tensors (scattered) |

**Key difference**: vLLM uses `cuMemcpyBatchAsync` (one driver call, N entries). llm-d uses a `cudaMemcpyAsync` loop (one call per tensor per block). With cross-layer layout the loop is trivial (1 tensor), so the overhead difference is minimal for factor=1.

---

## Metadata / Lookup Operations

| Operation | Frequency | Latency requirement |
|-----------|-----------|---------------------|
| `lookup(keys)` | Per request, per scheduling step | Must be fast (µs) |
| `prepare_load(keys)` | Once per prefix hit | Pins blocks from eviction |
| `touch(keys)` | Every scheduling step for active requests | Updates LRU |
| `prepare_store(keys)` | After each forward pass | May trigger eviction |

### Block identification

- Content-addressed by hash (`BlockHash = uint64`)
- llm-d file layout: `<base>_r<rank>/<hhh>/<hh>_g<group_idx>/<hash_hex>.bin` (2-level fanout with rank and group)
- Existence check: `os.path.exists()` per block (llm-d shared storage); C++ side uses `std::ifstream(path).good()`

---

## Eviction

| Backend | Policy | Notes |
|---------|--------|-------|
| CPU offloading | LRU or ARC | Ref-counted; pinned blocks immune |
| Shared storage (llm-d) | None (infinite) | External `pvc_evictor` sidecar handles GC |

---

## Device Tiers (llm-d scoring)

| Tier | Weight | Meaning |
|------|--------|---------|
| GPU | 1.0 | Block in GPU prefix cache (no I/O needed) |
| CPU | 0.8 | Block in CPU offload pool |
| SHARED_STORAGE | — | Block on shared filesystem/NVMe |

Scheduler uses longest-prefix-match scoring weighted by tier to route requests to pods with warmest caches.

---

## Key Implications for Certus

1. **Storage read unit is large** (~4–80 MB per blob) — NVMe sequential read is efficient here
2. **Reads are scattered across blobs** — N independent reads to N different content-hash locations, in parallel
3. **GPU↔DRAM handled by Certus directly** — Certus owns its own DMA buffers and calls `dma_copy_to_host`/`dma_copy_to_device` (does not use vLLM's CPU staging tensors)
4. **Write-once semantics** — no overwrites, no partial updates, enables aggressive caching/dedup
5. **Existence check already solved** — DispatchMap HashMap (u64 key, ~50ns, 0 I/O) handles per-block per-step lookups
6. **GDS opportunity** — llm-d supports `gds_mode` for direct GPU↔Storage (bypasses CPU staging)
7. **Concurrency is high** — up to 48 read-preferring workers per GPU (64 total threads, 0.75 read ratio)

---

## Certus: What's Already Built vs Remaining Gaps

### Already Implemented

| Capability | How Certus Handles It |
|------------|----------------------|
| **Existence check** | DispatchMap HashMap (u64 key, ~50ns, 0 I/O) |
| **Blob size alignment** | ExtentManager buddy allocator, configurable up to 1 GiB extents |
| **NVMe→DRAM→GPU read path** | `read_from_block_device()` → MDTS-segmented reads → DMA buffer → `gpu.dma_copy_to_device()` |
| **GPU→DRAM→NVMe write path** | `populate()` → `gpu.dma_copy_to_host()` → DMA staging → BackgroundWriter → MDTS-segmented writes |
| **Deferred write scheduling** | BackgroundWriter thread drains WriteJob channel asynchronously |
| **DRAM staging tier** | `create_staging()` allocates DMA buffers; entries live in staging until BackgroundWriter flushes to SSD |
| **Eviction** | Watermark-based `run_eviction_cycle()` with read/write ref-counting (pinned entries immune) |
| **Job tracking** | `store_async`/`load_async` track jobs by ID; `poll_completions()` returns `Vec<(job_id, success)>` |
| **Multi-drive striping** | Keys sharded across N data drives via `key % num_drives` |

### Remaining Gaps

| Gap | What vLLM/llm-d Does | What's Missing in Certus |
|-----|----------------------|--------------------------|
| **True async I/O** | 48+ concurrent reader threads | Dispatcher's `lookup()` issues synchronous `ReadSync` per segment. Needs SPDK async submission or parallel dispatch to saturate NVMe queue depth. Currently one read at a time per `lookup()` call. |
| **NVMe→GPU P2P DMA** | N/A (both go through CPU) | Certus goes NVMe→DRAM→GPU (same as vLLM/llm-d). P2P (NVMe→GPU BAR via SPDK) is a future optimization, not a gap vs current upstream. |
| **GDS mode** | llm-d supports `gds_mode` via cuFile | Not implemented. Optimization for bypassing CPU staging entirely. |
| **Cluster event propagation** | ZMQ pub/sub: `BlockStoredEvent`, `BlockRemovedEvent` | Single-node only. No event publishing. *(Multi-node only)* |
| **Router-side index** | Go InMemoryIndex: "which pods have block X?" | Not needed until multi-node. *(Multi-node only)* |

---

## See Also

- [kv_metadata_access.md](kv_metadata_access.md) — Detailed metadata access patterns, sizes, and frequency across vLLM, llm-d, and 3FS
