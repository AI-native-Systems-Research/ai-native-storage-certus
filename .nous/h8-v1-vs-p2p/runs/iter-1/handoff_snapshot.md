# Handoff — h8-v1-vs-p2p, Iteration 1

## Goal

Implement a P2P direct-read path in dispatcher v1 (`components/dispatcher/v1/`) that reads NVMe chunks directly into GPU BAR1 memory via `prepare_memory_for_spdk`, bypassing both the host ring buffers and per-chunk cudaMemcpy. Benchmark it against the existing pipelined bounce path through certus-server with `--dispatcher-version v1` and the Python client's `--bench` mode.

## Key Discoveries

1. **Dispatcher v1's pipelined bounce path** (`pipeline.rs:30-123`): Uses a ring of 4 host DMA buffers. For each of 32 chunks (4 MiB / 128 KiB), it: issues ReadSync into ring buffer → memcpy to memory-tier → `dma_copy_to_device` (cudaMemcpy H2D) to GPU. All steps serial per-chunk with ring overlap.

2. **`promote_and_serve` orchestration** (`lib.rs:190-266`): Called from the `BlockDevice` branch of `lookup()`. Evicts memory-tier entries if needed, inserts into memory-tier, reads from SSD via `pipelined_ssd_to_gpu`, then updates dispatch-map. The P2P path should SHORT-CIRCUIT this: skip memory-tier entirely, read directly to GPU.

3. **`prepare_memory_for_spdk` payload** (interface at `interfaces/src/igpu_services.rs:460-463`): Takes a base64 string encoding 72 bytes (64-byte `cudaIpcMemHandle_t.reserved` + 8-byte LE u64 size). Returns a DmaBuffer backed by GPU BAR1 registered with SPDK for direct NVMe DMA.

4. **ReadSync targets `buf.as_ptr()`** (`block-device-spdk-nvme/v2/src/actor.rs:1030`): The NVMe read command uses the DmaBuffer's pointer directly. Buffer length determines block count. Sub-DmaBuffer views at `gpu_ptr + offset` will work as long as the parent region was `spdk_mem_register`'d (which `prepare_memory_for_spdk` does).

5. **IpcHandle needs extension** (`interfaces/src/idispatcher.rs:113-118`): Currently only `address: *mut u8` and `size: u32`. Must add `cuda_ipc_handle_bytes: Option<Vec<u8>>` for the dispatcher to call `prepare_memory_for_spdk`.

6. **noop_free pattern exists** (`lib.rs:87`): `unsafe extern "C" fn noop_free(_ptr: *mut std::ffi::c_void) {}` — reuse for sub-buffer DmaBuffer views that must not free the parent's memory.

7. **Prior P2P data** (from h8-dispatcher-p2p standalone): Bounce 2206 MB/s, P2P-warm 3670 MB/s (1.66x). Those used BatchSubmit (parallel NVMe reads). Sequential ReadSync narrows the gap because NVMe can't overlap reads across chunks.

## System Interface

- **Build:**
  ```bash
  RUSTFLAGS='-L /usr/local/lib' cargo build -p certus-server
  ```
- **Run server (bounce baseline v1):**
  ```bash
  rm -f /var/tmp/spdk_pci_lock_0000:63:00.0 && \
  LD_LIBRARY_PATH=/usr/local/lib:/usr/local/cuda/lib64:$LD_LIBRARY_PATH \
  ./target/debug/certus-server \
    --metadata-pci 0000:63:00.0 \
    --data-pci 0000:63:00.0 \
    --dispatcher-version v1 \
    --listen 0.0.0.0:50051
  ```
- **Run client benchmark (4 MiB):**
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
- **Run client benchmark (4 KiB — control-negative):**
  ```bash
  cd apps/certus-server/python-client && \
  LD_LIBRARY_PATH=/usr/local/lib:/usr/local/cuda/lib64:$LD_LIBRARY_PATH \
  python3 test_client.py \
    --server localhost:50051 \
    --bench \
    --bench-object-size 4096 \
    --bench-num-objects 10 \
    --bench-iterations 20
  ```
- **Output format:** Stdout table: `Tier | Avg (us/obj) | Min (us/obj) | Max (us/obj) | Avg (GB/s) | Peak (GB/s)`. Parse "SSD-tier" row for the metric under test.
- **Baseline result:** Not measured yet (requires hardware). Reference: ~2206 MB/s from v0 bounce, expect v1 pipelined to be similar or slightly better.

## Code Map

| File:Line | What's there | When to look |
|-----------|-------------|--------------|
| `components/dispatcher/v1/src/pipeline.rs:30-123` | `pipelined_ssd_to_gpu` — the ring-buffer bounce path | Baseline implementation; study before writing P2P |
| `components/dispatcher/v1/src/pipeline.rs:16` | `PIPELINE_RING_SIZE = 4` | Understanding baseline ring design |
| `components/dispatcher/v1/src/lib.rs:190-266` | `promote_and_serve` — orchestrates eviction + SSD read + GPU serve | Where to add P2P routing branch |
| `components/dispatcher/v1/src/lib.rs:728-815` | `fn lookup` — dispatches MemoryTier/Staging/BlockDevice | Entry point for SSD lookups |
| `components/dispatcher/v1/src/lib.rs:805-809` | `LookupResult::BlockDevice` branch → calls `promote_and_serve` | The exact callsite |
| `components/dispatcher/v1/src/lib.rs:87` | `noop_free` function | Reuse for sub-buffer views |
| `components/dispatcher/v1/src/io_segmenter.rs:22-55` | `segment_io()` — splits transfer into 128 KiB segments | Used by both bounce and P2P paths |
| `components/interfaces/src/idispatcher.rs:113-118` | `IpcHandle` struct definition | Add cuda_ipc_handle_bytes field |
| `components/interfaces/src/igpu_services.rs:460-463` | `prepare_memory_for_spdk` trait method | P2P's key API call |
| `components/interfaces/src/spdk_types.rs:293-316` | `DmaBuffer::from_raw` | Create sub-buffer views |
| `components/interfaces/src/iblock_device.rs:187-194` | `Command::ReadSync` | NVMe read command format |
| `apps/certus-server/src/service.rs:176-254` | Lookup gRPC handler | Pass cuda_ipc_handle_bytes through |
| `apps/certus-server/src/service.rs:233` | IpcHandle construction in lookup | Add new field here |
| `apps/certus-server/python-client/test_client.py:310-439` | `bench_lookup_latency` function | Benchmark measurement flow |
| `components/block-device-spdk-nvme/v2/src/actor.rs:1030` | `buf_ptr = buf_guard.as_ptr()` | Confirms SPDK reads at buffer's ptr |
| `components/gpu-services/v0/src/lib.rs:330-479` | `prepare_memory_for_spdk` implementation | What happens with the base64 payload |

## Code Targets

### 1. IpcHandle extension (interfaces/src/idispatcher.rs:113-118)
Add `pub cuda_ipc_handle_bytes: Option<Vec<u8>>` field. Update doc example to include `cuda_ipc_handle_bytes: None`. All existing IpcHandle construction sites in the codebase must be updated to include the new field.

### 2. Service.rs lookup handler (apps/certus-server/src/service.rs:233)
Change the IpcHandle construction at line 233 to:
```rust
let ipc = IpcHandle {
    address: dev_ptr as *mut u8,
    size: handle.size,
    cuda_ipc_handle_bytes: Some(handle.cuda_ipc_handle.clone()),
};
```
This passes raw bytes through without any other logic change.

### 3. New P2P function (components/dispatcher/v1/src/pipeline.rs)
Add `pub unsafe fn p2p_ssd_to_gpu(drive, gpu, cuda_ipc_handle_bytes, gpu_dst, start_lba, total_bytes, numa_node)`:
- Construct 72-byte payload: `cuda_ipc_handle_bytes (64) ++ (total_bytes as u64).to_le_bytes() (8)`
- base64::engine::general_purpose::STANDARD.encode(payload)
- Call `gpu.prepare_memory_for_spdk(&encoded, None)` → get `gpu_dma_buf: DmaBuffer`
- segment_io() → for each segment, create `DmaBuffer::from_raw(gpu_dma_buf.as_ptr().add(seg.buffer_offset), seg.length, noop_free, numa_node)` → issue ReadSync → forget sub-buffer
- Drop `gpu_dma_buf` at end (SPDK unregister + IPC close)

### 4. Routing in promote_and_serve (components/dispatcher/v1/src/lib.rs:240-253)
Before the existing `pipelined_ssd_to_gpu` call, check `ipc_handle.cuda_ipc_handle_bytes.is_some()`. If yes:
- Do NOT insert into memory-tier (skip lines 202-207)
- Call `pipeline::p2p_ssd_to_gpu` instead
- Do NOT update dispatch-map (skip lines 258-263)
- Return Ok(())

### 5. Cargo.toml (components/dispatcher/v1/Cargo.toml)
Add `base64 = "0.22"` under [dependencies].

## What I Tried That Didn't Work

- **Explore sub-agents**: Authentication errors with haiku model. Used direct tool calls instead.
- **P2P with memory-tier promotion**: Copying from GPU BAR1 to host DRAM (for memory-tier) requires uncacheable MMIO reads (~1 GB/s), making it slower than bounce. Abandoned in favor of skipping memory-tier entirely.
- **Dual-read approach**: Reading the same data twice (once to host for memory-tier, once to GPU for serving) doubles NVMe utilization. Not viable for sequential submission.

## What I Excluded and Why

- **Memory-tier promotion in P2P path**: The benchmark measures cold (SSD) lookups repeatedly. Skipping memory-tier means each iteration hits SSD again — which is what we want for measuring the SSD path. A production P2P path would need a separate strategy for caching, but for this experiment it's unnecessary.
- **BatchSubmit / async reads**: The research question specifies sequential ReadSync. Parallelism is a separate dimension.
- **gpu-p2p-server modifications**: Explicitly forbidden by campaign constraints.
- **Standalone benchmarks**: Forbidden — must use certus-server + python-client.
- **Object sizes other than 4MiB and 4KiB**: 4 MiB tests the primary hypothesis (32 chunks of cudaMemcpy savings). 4 KiB tests the control (setup overhead dominates for 1 chunk).

## Evolution of Thinking

1. **Initial assumption**: P2P would read to GPU and also promote to memory-tier. Reality: copying FROM GPU BAR1 to host DRAM is extremely slow (uncacheable MMIO), making hybrid P2P+memory-tier promotion worse than pure bounce.

2. **Revised approach**: P2P path skips memory-tier entirely. For benchmarking cold lookups, this is fine — the benchmark accesses cold keys repeatedly, so each iteration hits SSD regardless. The memory-tier promotion is overhead we can eliminate.

3. **Sub-buffer correctness**: ReadSync reads into `buf.as_ptr()` for `buf.len() / sector_size` blocks. Creating a DmaBuffer sub-view at `gpu_ptr + offset` with length 128KiB will cause SPDK to DMA exactly 128KiB to that GPU address. The parent `prepare_memory_for_spdk` call registered the entire 4MiB GPU region with SPDK's IOMMU, so sub-pointers resolve correctly via `spdk_vtophys`.

4. **noop_free for sub-buffers is safe**: The sub-buffer's `Drop` calls `noop_free` (does nothing). We `std::mem::forget` them after ReadSync completes anyway, but even if we didn't, the parent GPU DmaBuffer handles SPDK unregistration on its own drop.

## Current Status

- **Validated:** Code paths understood, interface changes designed, build commands from v0 handoff apply (same workspace), prior experiment data available as reference
- **Uncertain:** Whether sub-buffer DmaBuffer views correctly produce physical addresses for SPDK vtophys (the registration covers the base address + full size, but SPDK may only track the registered base — sub-pointers within might work or might fail). If this fails, fallback: allocate individual 128KiB GPU DmaBuffers per chunk.
- **Suggested next:**
  - If P2P is faster: Measure per-call prepare_memory_for_spdk overhead (warm vs cold). Consider caching the GPU DmaBuffer across lookups for the same IPC handle.
  - If P2P is slower: Sub-buffer vtophys may be failing. Try individual prepare_memory_for_spdk calls per chunk (128 KiB each) — expensive but confirms mechanism.
  - If similar: The sequential NVMe submission is the true bottleneck (both paths wait for NVMe). Try BatchSubmit with P2P in next iteration.

## Warnings & Constraints

- **SPDK singleton**: Only one SPDK process per NVMe device. Kill certus-server before restarting: `pkill -f certus-server`
- **Lock file cleanup**: Always `rm -f /var/tmp/spdk_pci_lock_*` before starting
- **nvidia-peermem required**: `lsmod | grep nvidia_peermem` must show loaded. Without it, `spdk_mem_register` on GPU memory may silently succeed but DMA will go to wrong addresses.
- **gdrdrv required**: `lsmod | grep gdrdrv` — GDRCopy module needed for `prepare_memory_for_spdk`'s internal pinning/verification.
- **IpcHandle field addition breaks all construction sites**: After adding `cuda_ipc_handle_bytes`, every `IpcHandle { address, size }` in tests and production code needs `cuda_ipc_handle_bytes: None` appended. Grep for `IpcHandle {` to find all sites.
- **std::mem::forget sub-buffers**: Sub-DmaBuffer views must NOT be dropped via normal Drop (noop_free is harmless but forget is cleaner). The parent GPU DmaBuffer's drop handles cleanup.
- **P2P path skips memory-tier**: The benchmark must be designed so that skipping memory-tier doesn't affect measurement. The python client's SSD-tier benchmark measures cold lookups (objects evicted from memory-tier), so repeated lookups still hit SSD even without the skip — BUT only if the dispatch-map isn't updated to MemoryTier. The P2P path must leave the dispatch-map entry as BlockDevice.
- **Same session for valid comparison**: Run bounce and P2P conditions in the same session (back-to-back server starts with code swap) for comparable PCIe/NVMe conditions.
- **Debug build OK**: PCIe DMA latency (microseconds) dominates over CPU-bound work (nanoseconds). Release build won't meaningfully change ratios.
