# Handoff — h8-v1-pinned, Iteration 1

## Goal

Implement a P2P direct-read path in dispatcher v1 with **persistent GPU staging** — cache the `prepare_memory_for_spdk` result (GPU DmaBuffer registered with SPDK) across lookups to eliminate the per-lookup IPC open + SPDK register overhead that made P2P 17.5% slower in h8-v1-vs-p2p. Benchmark through certus-server with `--dispatcher-version v1` using the Python client's `--bench` mode.

## Key Discoveries

1. **Prior experiment failure cause**: h8-v1-vs-p2p called `prepare_memory_for_spdk` on EVERY lookup (~50μs: cudaIpcOpenMemHandle + spdk_mem_register). Over 10 objects × 20 iterations = 200 calls. This overhead exceeded the eliminated cudaMemcpy savings (~100-130μs per lookup for 32×128KiB H2D copies). Fix: cache the DmaBuffer.

2. **Memory-tier confound**: The bounce path promotes to memory-tier on first lookup (iterations 2-N serve from DRAM at ~328μs). The P2P path skips promotion, so all iterations hit SSD (~13000μs). Fix: compare first-iteration latency (both hit SSD cold) as the primary metric.

3. **prepare_memory_for_spdk payload format** (`gpu-services/v0/src/ipc.rs:11-37`): 72 bytes = 64 bytes CUDA IPC handle + 8 bytes u64 LE size, then base64-encoded as a string. Decoded at `ipc.rs:11`, size extracted at `ipc.rs:27`.

4. **Service layer already has handle bytes** (`service.rs:205`): The lookup handler extracts `handle_key: [u8; 64]` from `handle.cuda_ipc_handle` for its IPC cache. This is the same 64 bytes needed for the GPU DMA cache key. Just pass them through.

5. **ReadSync requires Arc<Mutex<DmaBuffer>>** (`interfaces/src/iblock_device.rs:187-194`): The Command::ReadSync variant takes `buf: Arc<Mutex<DmaBuffer>>`. Sub-buffer views for P2P must be wrapped in Arc<Mutex<>> before issuing.

6. **Sub-buffer views work with SPDK**: `spdk_mem_register` registers the full GPU buffer range. Sub-pointers within resolve via `spdk_vtophys` because SPDK's IOMMU mapping covers the entire region. The `noop_free` pattern (`lib.rs:87`) prevents sub-buffers from unregistering the parent.

7. **IpcHandle struct is minimal** (`interfaces/src/idispatcher.rs:112-118`): Only `address: *mut u8` and `size: u32`. Adding `cuda_ipc_handle_bytes: Option<Vec<u8>>` affects ~20+ construction sites (grep `IpcHandle {`). All non-P2P sites set it to `None`.

## System Interface

- **Build:**
  ```bash
  RUSTFLAGS='-L /usr/local/lib' cargo build -p certus-server
  ```
- **Run server:**
  ```bash
  rm -f /var/tmp/spdk_pci_lock_0000:63:00.0 && \
  LD_LIBRARY_PATH=/usr/local/lib:/usr/local/cuda/lib64:$LD_LIBRARY_PATH \
  ./target/debug/certus-server \
    --metadata-pci 0000:63:00.0 \
    --data-pci 0000:63:00.0 \
    --dispatcher-version v1 \
    --listen 0.0.0.0:50051
  ```
- **Run client benchmark (1 iteration — first-hit only):**
  ```bash
  cd apps/certus-server/python-client && \
  LD_LIBRARY_PATH=/usr/local/lib:/usr/local/cuda/lib64:$LD_LIBRARY_PATH \
  python3 test_client.py \
    --server localhost:50051 \
    --bench-only \
    --bench-object-size 4194304 \
    --bench-num-objects 10 \
    --bench-iterations 1
  ```
- **Run client benchmark (20 iterations):**
  ```bash
  cd apps/certus-server/python-client && \
  LD_LIBRARY_PATH=/usr/local/lib:/usr/local/cuda/lib64:$LD_LIBRARY_PATH \
  python3 test_client.py \
    --server localhost:50051 \
    --bench-only \
    --bench-object-size 4194304 \
    --bench-num-objects 10 \
    --bench-iterations 20
  ```
- **Output format:** Stdout table with columns: Tier, Avg (us/obj), Min (us/obj), Max (us/obj), Avg (GB/s), Peak (GB/s). Parse "SSD-tier" row.
- **Baseline result:** From h8-v1-vs-p2p: bounce SSD-tier avg 12969.1 μs/obj, 0.32 GB/s (4 MiB, 10 objects, 20 iterations).

## Code Map

| File:Line | What's there | When to look |
|-----------|-------------|--------------|
| `components/dispatcher/v1/src/pipeline.rs:30-123` | `pipelined_ssd_to_gpu` — existing ring-buffer bounce | Baseline reference; new P2P function goes after this |
| `components/dispatcher/v1/src/pipeline.rs:16` | `PIPELINE_RING_SIZE = 4` | Ring size constant |
| `components/dispatcher/v1/src/lib.rs:60-79` | `define_component!` block — fields section | Add `gpu_dma_cache` field here |
| `components/dispatcher/v1/src/lib.rs:87` | `noop_free` function | Reuse for GPU sub-buffer views |
| `components/dispatcher/v1/src/lib.rs:190-266` | `promote_and_serve` — SSD read orchestration | Add P2P routing branch before line 240 |
| `components/dispatcher/v1/src/lib.rs:728-815` | `fn lookup` — dispatches by LookupResult | Entry point; BlockDevice at 805 calls `promote_and_serve` |
| `components/dispatcher/v1/src/io_segmenter.rs:22-55` | `segment_io()` — splits into 128 KiB segments | Used by P2P function |
| `components/interfaces/src/idispatcher.rs:112-118` | `IpcHandle` struct | Add `cuda_ipc_handle_bytes` field |
| `components/interfaces/src/igpu_services.rs:460-463` | `prepare_memory_for_spdk` signature | Called on cache miss |
| `components/interfaces/src/iblock_device.rs:187-194` | `Command::ReadSync` variant | ReadSync takes `Arc<Mutex<DmaBuffer>>` |
| `components/interfaces/src/spdk_types.rs:293-316` | `DmaBuffer::from_raw` | Create GPU sub-buffer views |
| `apps/certus-server/src/service.rs:176-254` | Lookup gRPC handler | Pass cuda_ipc_handle_bytes |
| `apps/certus-server/src/service.rs:233` | IpcHandle construction in lookup | Add new field |
| `components/gpu-services/v0/src/lib.rs:330-483` | `prepare_memory_for_spdk` implementation | Full cost chain |
| `components/gpu-services/v0/src/ipc.rs:11-37` | `decode_ipc_payload` — base64 → (handle, size) | Payload format: 72 bytes = 64 handle + 8 LE size |
| `components/gpu-services/v0/src/dma.rs:114-162` | `create_spdk_dma_buffer_from_gpu` | spdk_mem_register logic |
| `apps/certus-server/python-client/test_client.py:310-439` | `bench_lookup_latency` | Benchmark flow |
| `apps/certus-server/python-client/test_client.py:344` | `lookup_tensor` allocation | Single GPU buffer reused |
| `apps/certus-server/python-client/test_client.py:347` | `lookup_ipc` construction | Same IPC handle for all lookups |

## Code Targets

### 1. IpcHandle extension (`components/interfaces/src/idispatcher.rs:112-118`)
Add `pub cuda_ipc_handle_bytes: Option<Vec<u8>>` after `size: u32`. Update doc example. All `IpcHandle {..}` sites need `cuda_ipc_handle_bytes: None` (or `Some(...)` for the lookup service path).

### 2. GPU DMA cache field (`components/dispatcher/v1/src/lib.rs:71-76`)
In `define_component!` fields block, add:
```rust
gpu_dma_cache: Mutex<HashMap<[u8; 64], Arc<Mutex<DmaBuffer>>>>,
```
Key: 64-byte CUDA IPC handle. Value: the DmaBuffer from `prepare_memory_for_spdk`.

### 3. Cache lookup method (`components/dispatcher/v1/src/lib.rs`, new impl method)
```rust
fn get_or_create_gpu_dma(
    &self,
    handle_bytes: &[u8],
    total_size: usize,
    gpu: &dyn IGpuServices,
) -> Result<Arc<Mutex<DmaBuffer>>, DispatcherError>
```
Construct 72-byte payload (64 handle + 8 LE size), base64 encode, call `gpu.prepare_memory_for_spdk(&encoded, None)`, wrap in Arc<Mutex>, insert into cache. Use `base64::engine::general_purpose::STANDARD`.

### 4. P2P function (`components/dispatcher/v1/src/pipeline.rs`, after line 123)
```rust
pub unsafe fn p2p_ssd_to_gpu_persistent(
    drive: &dyn IBlockDevice,
    gpu_dma_buf: &DmaBuffer,
    start_lba: u64,
    total_bytes: usize,
    numa_node: i32,
) -> Result<(), DispatcherError>
```
Uses `segment_io` → per-segment: create sub-DmaBuffer view via `from_raw(base_ptr + offset, length, noop_free, -1)` → wrap in Arc<Mutex<>> → ReadSync → await ReadDone → forget sub-buffer.

### 5. P2P routing in promote_and_serve (`components/dispatcher/v1/src/lib.rs:~240`)
Before existing `pipelined_ssd_to_gpu` call: check `ipc_handle.cuda_ipc_handle_bytes.is_some()`. If yes:
- Call `get_or_create_gpu_dma` with the handle bytes
- Lock the cached DmaBuffer to get base pointer
- Call `p2p_ssd_to_gpu_persistent`
- Do NOT insert into memory-tier, do NOT update dispatch-map
- `dm.release_write(key)` and return Ok(())

### 6. Service layer pass-through (`apps/certus-server/src/service.rs:233`)
Change to: `let ipc = IpcHandle { address: dev_ptr as *mut u8, size: handle.size, cuda_ipc_handle_bytes: Some(handle.cuda_ipc_handle.clone()) };`

### 7. Cargo.toml (`components/dispatcher/v1/Cargo.toml`)
Add `base64 = "0.22"` to [dependencies].

## What I Tried That Didn't Work

- **P2P without caching** (h8-v1-vs-p2p): 17.5% slower. Per-lookup `prepare_memory_for_spdk` (~50μs) × 200 calls exceeded the cudaMemcpy savings.
- **P2P with memory-tier promotion**: GPU BAR1 → host copy is uncacheable MMIO (~1 GB/s). Not viable.
- **Single bulk cudaMemcpy from memory-tier**: Just a minor bounce optimization, not P2P. Excluded.
- **4 KiB control-negative**: Already tested in h8-v1-vs-p2p iter-1. Confirmed P2P doesn't help for tiny objects.

## What I Excluded and Why

- **Memory-tier population in P2P path**: Benchmark measures cold SSD lookups. P2P leaves entries as BlockDevice so each iteration re-tests SSD→GPU. Production P2P would need separate strategy.
- **BatchSubmit/parallel reads**: Research question specifies sequential ReadSync. Parallelism for future iteration.
- **gpu-p2p-server modifications**: Forbidden by campaign constraints.
- **Release build**: PCIe DMA latency (microseconds) dominates CPU overhead (nanoseconds). Debug build doesn't change ratios.
- **Multi-NVMe striping**: Only one NVMe available. Not testable.

## Evolution of Thinking

1. **Starting assumption**: Prior P2P failure was solely due to per-call overhead. Investigation confirmed this (RP-7) — caching eliminates ~50μs × N_lookups cost.

2. **Effect size concern**: The 32 × cudaMemcpy savings (~100-130μs) against ~13000μs SSD time is only ~1%. This is within measurement noise for a single run. RP-5's "2x faster" likely reflects the standalone experiment's use of BatchSubmit (parallel NVMe reads reduce total time, making copy overhead a larger fraction). The sequential experiment may show smaller improvement.

3. **But the memcpy savings are additive**: The bounce path also does 32× `copy_nonoverlapping` to memory-tier slot. At ~0.5μs per 128KiB memcpy (L1 hit), that's ~16μs. Combined: 130+16 ≈ 150μs savings. Still ~1.1% of 13000μs.

4. **True P2P advantage may be in PCIe path**: If NVMe and GPU share a PCIe switch, P2P avoids root-complex traversal (saves 200-500ns per 128KiB transfer × 32 = 6-16μs). Marginal on top of copy savings.

5. **Accepted small-effect scenario**: If improvement is <3%, the experiment still provides critical data: confirms that sequential NVMe time dominates, and the next step is P2P + BatchSubmit (parallel NVMe reads where the P2P advantage compounds across concurrent transfers).

## Current Status

- **Validated:** Interface extension approach, build commands, benchmark client invocation, payload format (72 bytes = 64 handle + 8 LE size), ReadSync buffer semantics, sub-buffer view pattern
- **Uncertain:** Whether sequential P2P will show measurable improvement over bounce given NVMe time dominance. Effect may be <3%.
- **Suggested next:** If P2P persistent is only marginally faster (~1-3%): combine P2P + BatchSubmit (parallel NVMe reads to GPU sub-buffers). If P2P is still slower or equal: investigate PCIe topology (`nvidia-smi topo -m`, `lspci -tvvv`), verify spdk_vtophys returns valid addresses for GPU sub-offsets.

## Warnings & Constraints

- **SPDK singleton**: Only one SPDK process per NVMe. Kill certus-server before restarting: `pkill -f certus-server`
- **Lock file cleanup**: Always `rm -f /var/tmp/spdk_pci_lock_*` before starting
- **nvidia-peermem required**: `lsmod | grep nvidia_peermem` must show loaded
- **gdrdrv required**: `lsmod | grep gdrdrv` — GDRCopy needed for `prepare_memory_for_spdk`
- **IpcHandle field breaks all sites**: After adding `cuda_ipc_handle_bytes`, grep `IpcHandle {` — expect ~20+ sites in tests and production needing `cuda_ipc_handle_bytes: None`
- **Sub-buffer forget pattern**: After ReadSync completes on a sub-DmaBuffer view, `std::mem::forget(sub_buf)` prevents the noop_free from running. Technically harmless either way since noop_free does nothing, but avoids the overhead of the drop glue.
- **Cache entry lifetime**: The cache holds IPC handles and SPDK registrations for server lifetime. Safe because benchmark client keeps tensors alive for the full duration.
- **Same IPC handle across batch**: Benchmark pre-allocates one lookup tensor (`test_client.py:344`). Cache will have exactly 1 entry. The 4 MiB DmaBuffer accommodates all objects.
- **Condition C vs A at 20 iters is NOT a fair comparison**: Bounce warms memory-tier (iter 2-N from DRAM ~328μs). P2P stays on SSD (~13000μs). This is by design — compare only at 1 iteration for mechanism validation.
