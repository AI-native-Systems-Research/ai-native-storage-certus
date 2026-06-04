# Handoff: P2P GPUDirect Storage — Iteration 3

## Goal

Implement **direct NVMe→GPU data transfer** using nvidia-peermem (NOT GDRCopy BAR1 staging). Register the GPU IPC destination pointer with SPDK via `spdk_mem_register`, then read NVMe directly into GPU memory. This completely bypasses DRAM and eliminates the H2D copy. The GDRCopy BAR1 approach from iter-1/2 is abandoned — it is fundamentally broken (RP-1, RP-4).

## Key Discoveries

1. **Two completely different P2P mechanisms exist in the codebase:**
   - `create_spdk_dma_buffer_from_gpu_bar` (dma.rs:353) — GDRCopy path: pins GPU mem → maps BAR1 VA → registers BAR VA with SPDK. **BROKEN** for transfer use case (iter-2 proved no efficient CUDA path from BAR1 to VRAM).
   - `create_spdk_dma_buffer_from_gpu` (dma.rs:114) / `create_spdk_dma_buffer_from_cuda_malloc` (dma.rs:189) — nvidia-peermem path: calls `spdk_mem_register` directly on GPU device pointer. **NEVER TRIED** for cold lookup. The `prepare_memory_for_spdk` function (lib.rs:333) already validates this path works on IPC-opened pointers.

2. **nvidia-peermem is the correct mechanism** — When `spdk_mem_register` is called on a GPU device pointer, the nvidia-peermem kernel module calls `nvidia_p2p_get_pages` to expose the GPU memory's physical addresses to the IOMMU. SPDK's vtophys then resolves the GPU VA to a physical address that the NVMe controller can DMA to directly. No GDRCopy involvement.

3. **The dispatcher already has the pattern** — `pipelined_ssd_to_gpu_zero_copy` (pipeline.rs:244) creates noop-free DmaBuffer wrappers for chunks of a target buffer and reads NVMe into them via sliding-window. For the P2P path, we do the same but the target buffer is GPU memory instead of DRAM.

4. **No staging ring needed** — Data lands directly in the final GPU destination (the IPC-opened pointer from the client). No intermediate buffer, no copy. The sliding-window uses chunks of the destination itself.

5. **Registration overhead is acceptable** — `spdk_mem_register` on GPU memory is a one-time call per transfer (not per chunk). For 4 MiB objects with 1.7ms total transfer time, even 100µs registration overhead is <6%. And the elimination of H2D copy saves ~0.5-0.8ms.

6. **gpu-services::dma module is public** — The dispatcher can call `gpu_services::dma::create_spdk_dma_buffer_from_cuda_malloc` or directly declare `extern "C" fn spdk_mem_register`. The simplest path is to declare the FFI directly in pipeline.rs (same pattern as gpu-services/src/lib.rs:776).

7. **IPC-opened pointers work with spdk_mem_register** — `prepare_memory_for_spdk` (gpu-services/src/lib.rs:333) opens an IPC handle and passes the resulting pointer to `create_spdk_dma_buffer_from_gpu` which calls `spdk_mem_register`. This is the same pointer type we have in the cold lookup path (`ipc_handle.address`).

## System Interface

- **Build:** `cargo build -p certus-server --release --features p2p`
- **Run baseline:** `./target/release/certus-server --drive-count 1 --format` (wait 6s for port 50051)
- **Run P2P direct (no backfill):** `P2P_DIRECT=1 P2P_NO_BACKFILL=1 ./target/release/certus-server --drive-count 1 --format`
- **Run P2P direct (with backfill):** `P2P_DIRECT=1 ./target/release/certus-server --drive-count 1 --format`
- **Benchmark:** `python3 apps/python/certus-api-bench.py --clients 1 --num-objects 16 --iterations 10 --block-size 4194304`
- **Integrity:** `python3 apps/python/certus-api-bench.py --clients 1 --verify-integrity --integrity-objects 8`
- **Output format:** Stdout, look for `Lookup (cold)` section then `per-client=X.XX GB/s`
- **Baseline result:** 2.39 GB/s (iter-2 validated), p99=1950 us

## Code Map

| File:line | What's there | When to look |
|---|---|---|
| `components/dispatcher/src/pipeline.rs:244` | `pipelined_ssd_to_gpu_zero_copy` — sliding-window baseline | **Template for the new P2P direct function**: copy this structure exactly, replace mem_tier_ptr with gpu_dst, remove H2D copies, add spdk_mem_register/unregister |
| `components/dispatcher/src/pipeline.rs:274` | DmaBuffer wrappers for memory-tier chunks (noop_free) | **Same pattern for GPU chunks**: wrap each io_segmenter segment of gpu_dst as a noop-free DmaBuffer |
| `components/dispatcher/src/pipeline.rs:296` | Prime sliding-window with async reads | **Reuse exactly** — same pattern works for GPU-targeted DmaBuffers |
| `components/dispatcher/src/pipeline.rs:315` | Main loop: recv → submit next → issue H2D | **Modify**: recv → submit next → (no H2D needed, data already in GPU) |
| `components/dispatcher/src/pipeline.rs:226` | `noop_free` function | **Reuse** for the GPU destination DmaBuffer wrappers |
| `components/dispatcher/src/lib.rs:194` | `promote_and_serve` — cold lookup entry point | **Add P2P_DIRECT branch**: skip mt.insert, call new pipeline fn, register as BlockDevice-only |
| `components/dispatcher/src/lib.rs:1140` | `batch_lookup` cold path — per-entry loop | **Add P2P_DIRECT branch**: same as promote_and_serve |
| `components/gpu-services/src/dma.rs:12` | `extern "C" fn spdk_mem_register` declaration | **Copy to pipeline.rs** for the P2P direct path |
| `components/gpu-services/src/dma.rs:189` | `create_spdk_dma_buffer_from_cuda_malloc` | **Reference**: shows the pattern of spdk_mem_register on GPU memory |
| `components/gpu-services/src/lib.rs:333` | `prepare_memory_for_spdk` — full IPC→DmaBuffer flow | **Reference**: proves spdk_mem_register works on IPC-opened GPU pointers |
| `components/gpu-services/src/cuda_ffi.rs:118` | `cudaMemcpyAsync(dst, src, count, kind, stream)` | For h-ablation D2H backfill copy |
| `components/gpu-services/src/cuda_ffi.rs:65` | `CUDA_MEMCPY_DEVICE_TO_HOST = 2` | For h-ablation backfill direction |
| `components/dispatcher/src/io_segmenter.rs:22` | `segment_io` function | How chunks are computed from total_bytes |
| `components/dispatcher/Cargo.toml:24` | `p2p = ["gpu-services/p2p"]` feature | Confirms p2p feature propagation |

## Code Targets

### h-main: Direct NVMe→GPU (no DRAM)

1. **`components/dispatcher/src/pipeline.rs`** — Add `pipelined_ssd_to_gpu_direct` function.
   - Location: After `pipelined_ssd_to_gpu_zero_copy` (after line 401)
   - Feature gate: `#[cfg(feature = "p2p")]`
   - Add `extern "C" { fn spdk_mem_register(...) -> c_int; fn spdk_mem_unregister(...) -> c_int; }` at top of function or module level (guarded by `#[cfg(feature = "p2p")]`)
   - Structure: Mirror `pipelined_ssd_to_gpu_zero_copy` lines 244-401 but:
     - First call `spdk_mem_register(gpu_dst, total_bytes_aligned)` — fail if rc != 0
     - Create DmaBuffer wrappers targeting chunks of `gpu_dst` (not mem_tier_ptr)
     - Remove all `gpu.dma_copy_to_device_async` calls (data lands in GPU directly)
     - No CUDA streams needed for the main path (only for backfill)
     - After all completions: call `spdk_mem_unregister(gpu_dst, total_bytes_aligned)`
     - Forget DmaBuffer wrappers (noop_free pattern)
   - Signature: `unsafe fn pipelined_ssd_to_gpu_direct(drive: &dyn IBlockDevice, channels: &ClientChannels, gpu_dst: *mut std::ffi::c_void, start_lba: u64, total_bytes: usize, chunk_size: usize, max_queue_depth: usize, backfill: Option<(*mut u8, &dyn IGpuServices, &[GpuStream; 2])>) -> Result<(), DispatcherError>`
   - The `backfill` param: when Some, after each NVMe completion, issue cudaMemcpyAsync(D2H) from gpu_dst+offset to backfill_ptr+offset. When None, no extra copies.

2. **`components/dispatcher/src/lib.rs`** — Wire into cold lookup paths.
   - In `promote_and_serve` (line ~260): Check `std::env::var("P2P_DIRECT").is_ok()`. If set:
     - If also `P2P_NO_BACKFILL` is set: skip `mt.insert`, call `pipelined_ssd_to_gpu_direct(... backfill: None)`, then `dm.remove(key)` + register as BlockDevice entry at same offset (so future lookups re-read from SSD)
     - If `P2P_NO_BACKFILL` is NOT set: do `mt.insert` to get mem_ptr, call `pipelined_ssd_to_gpu_direct(... backfill: Some((mem_ptr, gpu, &streams)))`, register as MemoryTier
   - In `batch_lookup` cold loop (line ~1160): Same logic, using the thread-local streams and channels

### h-ablation: With DRAM backfill

3. **Same function `pipelined_ssd_to_gpu_direct`** with `backfill: Some(...)`:
   - After each NVMe completion for segment seg_idx:
     - Compute copy_len and buffer_offset from segments[seg_idx]
     - Issue `cudaMemcpyAsync(backfill_ptr + offset, gpu_dst + offset, copy_len, CUDA_MEMCPY_DEVICE_TO_HOST, stream)`
     - Periodically sync stream (every 16 completions)
   - Final sync of stream before returning

## What I Tried That Didn't Work

- **Iter-1: Batch-based P2P with GDRCopy BAR1 staging** — 19% slower than baseline due to serialized read→copy phases (RP-2).
- **Iter-2: Sliding-window P2P with GDRCopy BAR1 staging** — 140x slower (0.01 GB/s). D2D copies from BAR1 dev_ptr read stale GPU L2 cache (RP-4). H2D from BAR1 VA uses pageable memory path (~10ms/128KiB). Fundamentally broken (RP-1).
- **GDRCopy gdr_pin_buffer on IPC-opened pointers** — fails because nvidia_p2p_get_pages only works for memory allocated by the same process via cudaMalloc (this is a GDRCopy limitation, NOT a nvidia-peermem limitation).
- **cudaHostRegister on BAR1 VA** — makes performance worse (iter-2 diagnostic).
- **CLI flags `--metadata-pci`/`--data-pci`** — do not exist. Use `--drive-count 1`.

## What I Excluded and Why

- **Registration caching (persistent spdk_mem_register):** For simplicity, we register/unregister per cold lookup. If the overhead is significant, a future iteration can cache registrations. But the per-call cost should be <<1ms.
- **Multi-drive scaling:** Uses 1 drive to isolate the P2P mechanism. Multi-drive introduces PCIe contention that could mask the P2P benefit.
- **Larger ring/queue depths:** The sliding-window depth is already tuned in the baseline (max_queue_depth=16). No change needed.
- **cuFile/GDS library integration:** cuFile is NVIDIA's official GPUDirect Storage library. We're implementing the same mechanism manually (spdk_mem_register is what cuFile does internally for SPDK). No need for the external library.
- **PipelineRingP2P from iter-1:** This struct allocated a fixed ring of GDRCopy-mapped buffers. We don't need it — the new approach uses the GPU destination itself as the DMA target (no separate ring).

## Evolution of Thinking

1. Started by analyzing iter-2's failure: the GDRCopy BAR1 path is fundamentally broken because there's no efficient way to move data from BAR1-staged memory to the final GPU destination.
2. Realized there are **two completely separate mechanisms** for GPU memory DMA in the codebase: GDRCopy (BAR1 mapping) and nvidia-peermem (direct spdk_mem_register on GPU pointers). Iter-1/2 only used GDRCopy.
3. Found that `prepare_memory_for_spdk` (gpu-services/src/lib.rs:333) already validates the nvidia-peermem path works: it opens an IPC handle, calls `create_spdk_dma_buffer_from_gpu` which calls `spdk_mem_register` on the GPU pointer. This is a proven, working path.
4. Recognized the key insight: we don't need a staging ring at all. The `pipelined_ssd_to_gpu_zero_copy` function already demonstrates reading NVMe into arbitrary memory (it reads into the memory-tier DRAM slot). The same pattern works if we point it at GPU memory — we just need `spdk_mem_register` on the GPU pointer first.
5. The design becomes trivially simple: (a) register GPU memory with SPDK, (b) reuse the existing sliding-window read-into-chunks pattern targeting GPU instead of DRAM, (c) unregister after transfer. No GDRCopy, no BAR1, no staging, no copies.

## Current Status

- **Validated:** Build compiles with `--features p2p`. Baseline benchmark produces 2.39 GB/s. `spdk_mem_register` on GPU device pointers is proven working via `prepare_memory_for_spdk`. nvidia-peermem kernel module is loaded. The sliding-window pipeline pattern works correctly.
- **Uncertain:** (1) Whether `spdk_mem_register` works on the IPC-opened pointer in the dispatcher's context (different from gpu-services where `prepare_memory_for_spdk` is called — same process, should work). (2) Whether NVMe DMA to GPU memory achieves the same per-write throughput as to DRAM — PCIe posted writes to GPU should be similar speed, but GPU memory controller behavior may differ. (3) Per-call `spdk_mem_register/unregister` overhead — if >500µs, it could eat into the gains for small objects.
- **Suggested next:** If direct NVMe→GPU works and exceeds baseline: (1) Test multi-drive to approach PCIe bandwidth ceiling. (2) Test registration caching to amortize overhead. (3) Test with multiple clients. If it fails: investigate whether the IPC pointer needs special handling for spdk_mem_register, or whether a single cudaMalloc within the server process works (allocate server-side buffer, NVMe into it, then fast D2D to IPC destination).

## Warnings & Constraints

1. **Server must be killed and restarted between conditions** — env vars are read at init or per-call, but SPDK state persists.
2. **Wait 6+ seconds after server start** — SPDK init, NVMe probe, extent format all take time.
3. **nvidia-peermem kernel module required** — verify with `lsmod | grep nvidia_peermem`. If `spdk_mem_register` returns non-zero on GPU pointer, this module isn't loaded.
4. **DmaBuffer wrappers for GPU chunks must use `noop_free`** — the GPU memory is owned by the IPC handle (client process). Don't free it.
5. **`std::mem::forget` the noop-free DmaBuffer wrappers** — same pattern as `pipelined_ssd_to_gpu_zero_copy` line 396-398.
6. **Must `spdk_mem_unregister` before returning** — if the function errors partway through, unregister in the error path too.
7. **The `P2P_DIRECT` env var selects h-main/h-ablation behavior** — set it for both P2P conditions. `P2P_NO_BACKFILL` additionally selects h-main (no DRAM). Absence of `P2P_DIRECT` selects baseline.
8. **For h-main (no backfill), skip `mt.insert` entirely** — don't allocate DRAM that won't be used. After transfer, do NOT create a MemoryTier entry.
9. **Chunk size comes from `max_transfer_size` (typically 128 KiB)** — the pipeline ring's chunk_size. Access via `ring_guard.as_ref().map_or(131072, |r| r.chunk_size)`.
10. **spdk_mem_register size must be aligned** — align total_bytes up to block_size (same as aligned_bytes in the existing code).
