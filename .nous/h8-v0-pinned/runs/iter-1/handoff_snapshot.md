# Handoff — h8-v0-pinned, Iteration 1

## Goal

Implement a P2P read path in dispatcher v0 with **persistent GPU staging** (pre-pinned buffer reused across the entire lookup batch), then benchmark it against the existing bounce path. All tests run through certus-server with `--dispatcher-version v0`. The key difference from h8-v0-vs-p2p is that `prepare_memory_for_spdk` is called ONCE per batch (not per lookup), amortizing the 4-5ms setup overhead that previously made P2P 33% slower.

## Key Discoveries

1. **Previous experiment (h8-v0-vs-p2p iter-1) showed P2P is 33% slower** — bounce 13764 us/obj vs P2P 18372 us/obj for 4 MiB. Root cause: `prepare_memory_for_spdk` called per-lookup adds cudaIpcOpenMemHandle + spdk_mem_register overhead (~4-5ms each call).

2. **Pre-pinning amortization math**: With 10 objects per tier and 20 iterations, one `prepare_memory_for_spdk` call (~5ms) amortized over 10×20=200 lookups = 0.025ms per lookup (negligible). This should reveal the raw DMA path difference.

3. **Sub-buffer DMA views work**: The previous experiment validated that `DmaBuffer::from_raw(gpu_base_ptr + chunk_offset, 128KiB, noop_free, -1)` works correctly with SPDK DMA. NVMe targets the correct GPU BAR1 physical address.

4. **Promotion requirement adds complexity**: The P2P path must also copy data to host DRAM for subsequent lookup caching. This means P2P does: NVMe→GPU + host copy for promotion. Bounce does: NVMe→host + GPU copy. Same total data moved, different order. The ablation arm tests whether promotion negates the path advantage.

5. **Python client benchmark reuses a single GPU buffer across all lookups** (`test_client.py:344-347`): It allocates one `lookup_tensor` and reuses its IPC handle. This means the server receives the same CUDA IPC handle bytes for all entries in a batch — ideal for the pre-pinning approach.

6. **certus-server lookup handler opens IPC handles with caching** (`service.rs:189`): For the bounce path, it caches opened handles within a batch (HashMap keyed by 64-byte handle). For P2P, we replace this with a single `prepare_memory_for_spdk` call at batch start.

7. **4 MiB = 32 chunks of 128 KiB** at default MDTS. Sequential ReadSync: each chunk is ~0.3-0.4ms (from the 13ms total / 32 chunks in baseline). The final cudaMemcpy for 4 MiB is ~1-2ms. P2P eliminates this final copy but the individual chunk DMA-to-GPU-BAR1 may be slightly slower per chunk due to BAR1 write combining behavior.

## System Interface

- **Build:**
  ```bash
  RUSTFLAGS='-L /usr/local/lib' cargo build -p certus-server
  ```
- **Run server (bounce baseline):**
  ```bash
  rm -f /var/tmp/spdk_pci_lock_0000:63:00.0 && \
  LD_LIBRARY_PATH=/usr/local/lib:/usr/local/cuda/lib64:$LD_LIBRARY_PATH \
  ./target/debug/certus-server \
    --metadata-pci 0000:63:00.0 \
    --data-pci 0000:63:00.0 \
    --dispatcher-version v0 \
    --listen 0.0.0.0:50051
  ```
- **Run client benchmark:**
  ```bash
  cd apps/certus-server/python-client && \
  LD_LIBRARY_PATH=/usr/local/lib:/usr/local/cuda/lib64:$LD_LIBRARY_PATH \
  python3 test_client.py \
    --server localhost:50051 \
    --bench \
    --bench-object-size 4194304 \
    --bench-num-objects 10 \
    --bench-iterations 20
  ```
- **Output format:** Stdout table: Tier | Avg (us/obj) | Min (us/obj) | Max (us/obj) | Avg (GB/s) | Peak (GB/s). Parse "SSD-tier" row.
- **Baseline result:** SSD-tier 13763.8 us/obj (0.30 GB/s) from h8-v0-vs-p2p iter-1.

## Code Map

| File:Line | What's there | When to look |
|-----------|-------------|--------------|
| `components/dispatcher/v0/src/lib.rs:179-276` | `read_from_block_device` — bounce path (sequential ReadSync + dma_copy_to_device) | Primary target: implement P2P alternative after this method |
| `components/dispatcher/v0/src/lib.rs:694-747` | `IDispatcher::lookup` — routes to staging vs block-device path | Add P2P routing: if `cuda_ipc_handle_bytes.is_some()`, call P2P method |
| `components/dispatcher/v0/src/lib.rs:218-263` | Sequential ReadSync loop (per-chunk pattern) | Replicate for P2P but with GPU sub-view buffers |
| `components/dispatcher/v0/src/lib.rs:266-273` | `gpu.dma_copy_to_device` — the final H2D copy P2P eliminates | Verify this is NOT called in P2P path |
| `components/interfaces/src/idispatcher.rs:113-118` | `IpcHandle` struct (currently: address + size only) | Add `cuda_ipc_handle_bytes: Option<Vec<u8>>` field |
| `apps/certus-server/src/service.rs:176-253` | Lookup gRPC handler with IPC handle caching | Replace with single prepare_memory_for_spdk at batch start |
| `apps/certus-server/src/service.rs:65-93` | `open_cuda_ipc` function | Still needed for populate and bounce control |
| `components/gpu-services/v0/src/lib.rs:330-479` | `prepare_memory_for_spdk` implementation | Understanding the 72-byte payload format and what it returns |
| `components/dispatcher/v0/src/io_segmenter.rs:22-55` | `segment_io` function | Generates 32 segments for 4 MiB at 128 KiB MDTS |
| `apps/certus-server/python-client/test_client.py:310-439` | `bench_lookup_latency` function | Full benchmark flow — reuses single GPU buffer |
| `components/dispatcher/v0/Cargo.toml` | Dependencies | Add `base64 = "0.22"` |

## Code Targets

### 1. IpcHandle extension (`components/interfaces/src/idispatcher.rs:113-118`)
Add `pub cuda_ipc_handle_bytes: Option<Vec<u8>>` field. Update doc example at line 108. All existing construction sites use `cuda_ipc_handle_bytes: None`. This is identical to what h8-v0-vs-p2p did — same patch applies.

### 2. Service.rs Lookup handler (`apps/certus-server/src/service.rs:176-253`)
**Key difference from h8-v0-vs-p2p**: Instead of removing the IPC cache and passing raw bytes through (which causes per-lookup prepare_memory_for_spdk), we:
- At batch start: take the first entry's 64-byte handle, build the 72-byte payload (handle + size as LE u64), base64-encode, call `prepare_memory_for_spdk` → get persistent GPU DmaBuffer
- For each entry: pass `cuda_ipc_handle_bytes: Some(bytes)` AND `address: dev_ptr` (the opened pointer from prepare's internal cudaIpcOpen)
- After batch: drop the GPU DmaBuffer (triggers SPDK unregister + IPC close via Drop)

Wait — the `prepare_memory_for_spdk` return type is `DmaBuffer` which doesn't expose the GPU device pointer separately. The service layer needs BOTH:
1. The DmaBuffer (for P2P reads in the dispatcher)
2. The GPU device pointer (for dma_copy_to_device in staging path)

**Alternative approach**: The service layer pre-opens the CUDA IPC handle (same as current bounce path), AND passes the raw bytes. The dispatcher calls prepare_memory_for_spdk once per batch (detected by having a cached buffer). This way the address field is valid for staging lookups that use dma_copy_to_device, and the P2P path uses the pre-pinned buffer.

**Simplest approach**: Keep the service layer mostly as-is (open IPC handle once via cache), pass raw bytes through. In the **dispatcher**, add a "P2P staging buffer" field or per-batch cache: on first BlockDevice lookup with handle bytes, call prepare_memory_for_spdk and cache the result. Subsequent lookups in the same batch reuse it. The Python client benchmark reuses the same handle, so the cache hit rate is 100%.

### 3. Dispatcher P2P-pinned read method (`components/dispatcher/v0/src/lib.rs`, after line 276)
New method `read_from_block_device_p2p_pinned`:
- Check if a cached GPU DmaBuffer exists for this IPC handle; if not, call prepare_memory_for_spdk to create one
- Cache the buffer (keyed by the 64-byte handle bytes)
- Run io_segmenter → for each segment, create sub-view via `DmaBuffer::from_raw(gpu_ptr + offset, length, noop_free, -1)`
- Issue ReadSync into each sub-view sequentially
- After all chunks complete: optionally copy data from a host buffer to staging (for promotion)
- Return Ok(())

### 4. P2P buffer cache (`components/dispatcher/v0/src/lib.rs`, fields section)
Add a `Mutex<HashMap<[u8; 64], Arc<DmaBuffer>>>` field to DispatcherComponentV0 (via define_component! fields). This caches prepared GPU buffers keyed by CUDA IPC handle bytes. First lookup creates it; subsequent lookups reuse it.

### 5. Cargo.toml (`components/dispatcher/v0/Cargo.toml`)
Add `base64 = "0.22"` to `[dependencies]`.

## What I Tried That Didn't Work

- **Explore agents failed** due to model authentication errors (only certain models available). Used direct tool calls instead.
- **Previous approach (h8-v0-vs-p2p)**: Calling prepare_memory_for_spdk per-lookup made P2P 33% slower. The per-call cost dominates. This iteration fixes that by caching.

## What I Excluded and Why

- **BatchSubmit / pipelined reads**: The research question is specifically about sequential ReadSync. Pipelining is a separate dimension (tested in h8-dispatcher-p2p). We isolate the DMA path variable.
- **Larger/smaller object sizes in this iteration**: Focus on 4 MiB first. The previous experiment already tested 1 MiB (same direction, smaller effect). Add robustness after the primary mechanism is validated.
- **Memory-tier integration**: v0 dispatcher doesn't have a memory-tier. "Promotion" means keeping data in the dispatch map staging buffer for subsequent lookups. This is simpler than full memory-tier integration.
- **gpu-p2p-server modifications**: Explicitly forbidden by campaign constraints.
- **Standalone benchmark binaries**: Explicitly forbidden by campaign constraints.
- **Release build**: Debug build is acceptable because PCIe DMA latency (~us) dominates over CPU instruction overhead (~ns). Release might shave 100-200us but doesn't change conclusions.

## Evolution of Thinking

1. **Started with**: "Just pre-pin at batch start in the service layer." But realized `prepare_memory_for_spdk` returns a DmaBuffer that the *dispatcher* needs for ReadSync, not the service layer. The service layer and dispatcher are separate.

2. **Key insight**: The Python client benchmark uses a single GPU buffer for all lookups in a tier. The CUDA IPC handle is the same for all entries. This means a simple cache in the dispatcher (keyed by handle bytes) achieves 100% hit rate after the first lookup. The first lookup pays ~5ms setup; all subsequent lookups are free.

3. **Promotion clarification**: "Promote to memory-tier" in v0 context means: after P2P read to GPU, also have the data available in host DRAM for subsequent lookups. Two options: (a) read from SSD to host buffer simultaneously with P2P (doubles bandwidth use), or (b) copy GPU→host after P2P completes (uses PCIe bandwidth). Option (b) uses `dma_copy_to_host` which the gpu_services interface already provides.

4. **Ablation value**: The ablation arm (P2P without promotion) isolates whether the promotion copy cancels the path advantage. If P2P-no-promotion is fast but P2P-with-promotion matches bounce, then the total data movement is equivalent and there's no net benefit to P2P for v0.

## Current Status

- **Validated:** Build commands work, baseline data available from prior experiment (13764 us/obj bounce), sub-buffer approach confirmed working, Python client reuses single GPU handle (ideal for caching)
- **Uncertain:** Whether NVMe DMA to GPU BAR1 at 128 KiB granularity achieves the same per-chunk throughput as NVMe DMA to host DRAM. BAR1 writes may be slower due to PCIe bridge characteristics.
- **Suggested next:**
  - If P2P-pinned is faster: Quantify how much faster and compare against standalone p2p-server numbers (1.66x). If significantly less, the sequential submission is the bottleneck → next iteration should test pipelined P2P.
  - If P2P-pinned matches bounce: The DMA path advantage is real but exactly cancelled by the promotion copy. Consider whether promotion can be deferred or skipped for certain access patterns.
  - If P2P-pinned is still slower: BAR1 write overhead per 128 KiB chunk is inherently higher than host DRAM DMA. P2P only wins with larger chunk sizes or batched submission. No further sequential P2P experiments needed.

## Warnings & Constraints

- **SPDK singleton**: Only one SPDK process per NVMe device. Kill certus-server before restarting: `pkill certus-server; sleep 2`
- **Lock file cleanup**: Always `rm -f /var/tmp/spdk_pci_lock_*` before starting server.
- **nvidia_peermem required**: `lsmod | grep nvidia_peermem` must show loaded. Without it, NVMe DMA to GPU BAR1 fails silently or returns garbage.
- **gdrdrv required**: `lsmod | grep gdrdrv` — needed for `prepare_memory_for_spdk`.
- **Device 0000:62:00.0 has VFIO issues**: Only use 0000:63:00.0.
- **DmaBuffer sub-views (from_raw at offset) must NOT be dropped before parent**: Use `noop_free` for sub-views. The parent DmaBuffer (from prepare_memory_for_spdk) handles SPDK unregistration on drop.
- **P2P buffer cache must be cleared on shutdown**: Add cleanup in the dispatcher's `shutdown()` method to drop cached GPU DmaBuffers before SPDK env is destroyed.
- **IpcHandle field addition breaks all construction sites**: Update test code in `lib.rs:1385-1389` and all other places that construct IpcHandle to include `cuda_ipc_handle_bytes: None`.
- **prepare_memory_for_spdk payload format**: 72 bytes = 64-byte cudaIpcMemHandle_t.reserved + 8-byte LE u64 size. Base64-encoded. Documented at `gpu-services/v0/specs/002-gpu-ssd-dma-prepare/contracts/igpu_services_prepare.md`.
- **Same session for valid comparison**: Run bounce and P2P conditions back-to-back without rebooting. NVMe warm-up matters for first-run latency.
- **Python client bench uses batch_size=50 for populate** (`test_client.py:352-353`): Lookup batches are `num_objects` (10) entries. The P2P cache only needs to handle 10 concurrent lookups with the same handle.
