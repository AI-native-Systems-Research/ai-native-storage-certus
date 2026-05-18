# KV Cache I/O Access Patterns

Summary of access patterns Certus must support, derived from vLLM (v0.12+) and llm-d-kv-cache source code.

---

## Transfer Granularity: Old vs New Layout

The cross-layer layout (PR #27743, vLLM 0.12.0) consolidated per-layer fragments into one contiguous block.

| Model | Old (per-layer, K or V) | New (cross-layer, all layers) | Improvement |
|-------|------------------------|-------------------------------|-------------|
| Llama-3.1-8B (TP=1) | 32 KB | 2.0 MB | 64× |
| Llama-3.1-70B (TP=1) | 32 KB | 5.0 MB | 160× |
| Llama-3.1-70B (TP=8) | 8 KB | 0.625 MB | 80× |
| DeepSeek-V3 MLA (TP=1) | 18 KB | 2.14 MB | 122× |
| Qwen2.5-7B (TP=1) | 16 KB | 0.88 MB | 56× |
| Gemma-2-27B (TP=1) | 64 KB | 5.75 MB | 92× |

**Formula (new):** `gpu_page = 2 × block_size(16) × num_kv_heads × head_dim × dtype_bytes × num_layers`

**Formula (old):** `half_page = block_size(16) × num_kv_heads × head_dim × dtype_bytes` (one layer, K or V)

---

## Offloaded Block = Storage I/O Unit

One offloaded block = `block_size_factor` GPU blocks bundled together for storage.

| Parameter | Typical Value |
|-----------|---------------|
| GPU block size | 16 tokens |
| Offloaded block size | 256 tokens (configurable) |
| block_size_factor | 16 (= 256 / 16) |
| File size (llm-d) | gpu_page × 16 |

| Model | File / offloaded block size |
|-------|----------------------------|
| Llama-8B TP=1 | 32 MB |
| Llama-70B TP=1 | 80 MB |
| Llama-70B TP=8 | 10 MB |
| Llama-8B TP=8 | 4 MB |

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

### Write data flow

```
GPU blocks (scattered)
  → cuMemcpyBatchAsync (16 entries × ~2 MB each, contiguous src per entry)
  → CPU staging buffer (pinned, contiguous per offloaded block)
  → Storage write (one blob per content-hash)
```

---

## Read Pattern (Storage → GPU)

| Property | Description |
|----------|-------------|
| Trigger | Scheduler detects prefix hit in offloaded cache |
| Granularity | One offloaded block per storage read |
| Access pattern | Prefix-sequential (always longest contiguous prefix from a start offset) |
| Concurrency | Up to ~48 concurrent reader threads (64 threads × 75% read ratio) |
| Fan-out | N independent reads in parallel (one per offloaded block / file) |
| Latency sensitivity | Blocks token generation start; on critical path |

### Read data flow

```
Storage (N parallel reads of independent blobs, ~4–80 MB each)
  → CPU staging buffer (thread-local pinned memory)
  → cuMemcpyBatchAsync (16 entries × ~2 MB each)
  → GPU blocks (scattered destination block IDs)
```

---

## DMA Transfer Patterns (GPU ↔ CPU Staging)

### vLLM CPU Offloading (`gpu_worker.py` → `swap_blocks_batch`)

| Property | Value |
|----------|-------|
| API | `cuMemcpyBatchAsync` (CUDA 12.8+), fallback loop of `cudaMemcpyAsync` |
| Entries per batch | block_size_factor (16) × num_groups × blocks_in_job |
| Per-entry size | `data_ref.page_size_bytes` = gpu_page_size (~0.5–5 MB) |
| Submission | Single driver call for all entries |
| Source (load) | Contiguous sub-blocks within CPU pinned buffer |
| Destination (load) | Scattered GPU block indices (non-contiguous) |
| Ordering | Each job's CUDA stream waits on the previous job's end_event |

### llm-d fs_backend (`tensor_copier.cu` → `copy_blocks_via_cuda_memcpy`)

| Property | Cross-layer layout (new) | Per-layer layout (old) |
|----------|-------------------------|----------------------|
| API | `cudaMemcpyAsync` in loop | `cudaMemcpyAsync` in loop |
| Calls per offloaded block | 16 (= gpu_blocks_per_file) | 16 × num_layers |
| Per-call size | `m_tensor_block_size` (~2 MB) | `m_tensor_block_size` (~32 KB) |
| Total per offloaded block | ~32 MB | ~32 MB (same data, more calls) |
| Source (load) | Contiguous CPU staging buffer | Contiguous CPU staging buffer |
| Destination (load) | Single GPU tensor (cross-layer) | Per-layer GPU tensors (scattered) |

**Key difference**: vLLM uses `cuMemcpyBatchAsync` (one driver call, N entries). llm-d uses a `cudaMemcpyAsync` loop (N driver calls, one entry each). Both achieve the same transfer but vLLM amortizes driver overhead better.

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
- llm-d file layout: `<base>/<hhh>/<hh>/<hash_hex>.bin` (2-level fanout)
- Existence check: `os.path.exists()` per block (llm-d shared storage)

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
3. **GPU scatter is handled by CUDA DMA** — Certus only needs to deliver contiguous blobs to CPU staging
4. **Write-once semantics** — no overwrites, no partial updates, enables aggressive caching/dedup
5. **Existence check already solved** — DispatchMap HashMap (u64 key, ~50ns, 0 I/O) handles per-block per-step lookups
6. **GDS opportunity** — llm-d supports `gds_mode` for direct GPU↔Storage (bypasses CPU staging)
7. **Concurrency is high** — 48+ reader threads hitting storage simultaneously per GPU

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
