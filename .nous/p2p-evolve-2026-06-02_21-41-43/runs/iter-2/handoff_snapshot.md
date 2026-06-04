# Handoff: P2P GPUDirect Storage — Iteration 2

## Goal

Implement a **sliding-window** P2P pipeline (NVMe→GPU BAR1 with overlapped D2D copies) that fixes iter-1's batch-based approach. The batch approach lost 19% throughput vs baseline due to serialized read→copy phases. The sliding-window overlaps NVMe reads with D2D copies, matching the baseline's structure while eliminating one PCIe crossing.

## Key Discoveries

1. **Iter-1's failure was structural, not fundamental** — The batch-based P2P pipeline (submit 8 reads → wait all → D2D copy all → sync) serialized reads and copies. The baseline's `pipelined_ssd_to_gpu_zero_copy` (pipeline.rs:244) uses a sliding-window (VecDeque of in-flight indices) that overlaps reads with copies. The fix is to use the same sliding-window pattern for P2P.

2. **D2D copies are effectively free** — `cudaMemcpyAsync(DeviceToDevice)` between BAR1-backed memory and VRAM on the same GPU uses internal GPU fabric (~1.5 TB/s on A30). For 128 KiB chunks: ~0.085 µs vs ~22 µs for each NVMe read. Ring slot reuse is never a bottleneck with 8 slots.

3. **The baseline does NOT use `ptr::copy_nonoverlapping`** — The zero-copy path (pipeline.rs:244) reads NVMe directly into the memory-tier slot and issues H2D from the same memory. No intermediate memcpy. The only data movements are: NVMe→DRAM (PCIe) + DRAM→GPU H2D (PCIe). P2P replaces this with: NVMe→GPU BAR1 (PCIe) + D2D on-GPU (free).

4. **GDRCopy cannot pin IPC-opened pointers** — `gdr_pin_buffer` calls `nvidia_p2p_get_pages` which only works for memory allocated by the same process via `cudaMalloc`. The final GPU destination (from `cudaIpcOpenMemHandle`) cannot be pinned. A staging ring buffer is mandatory.

5. **Periodic stream sync pattern** — The baseline syncs every 16 completions (pipeline.rs:381). For the P2P path with 8 ring slots, sync every `ring_size` completions is sufficient since D2D completes ~200x faster than the NVMe read that fills the next slot.

6. **Environment variable control works** — Iter-1 successfully used `P2P_NO_BACKFILL` env var to switch between backfill/no-backfill modes at runtime. Reuse this pattern.

7. **The P2P ring allocation primitives from iter-1 work** — `create_p2p_ring_slot` (cudaMalloc + GDRCopy pin + BAR1 map + SPDK register) was validated: NVMe reads into BAR1 succeeded with correct data integrity. Only the pipeline scheduling logic needs rewriting.

## System Interface

- **Build:** `cargo build -p certus-server --release --features p2p`
- **Run baseline:** `./target/release/certus-server --drive-count 1 --format` (wait 4+ seconds for port 50051)
- **Run P2P (no backfill):** `P2P_NO_BACKFILL=1 ./target/release/certus-server --drive-count 1 --format`
- **Run P2P (with backfill):** `./target/release/certus-server --drive-count 1 --format` (default when P2P code is active)
- **Benchmark:** `python3 apps/python/certus-api-bench.py --clients 1 --num-objects 16 --iterations 10 --block-size 4194304`
- **Integrity:** `python3 apps/python/certus-api-bench.py --clients 1 --verify-integrity --integrity-objects 8`
- **Output format:** Stdout, look for `Lookup (cold)` section then `per-client=X.XX GB/s`
- **Baseline result:** 2.41–2.43 GB/s (iter-1 validated)

## Code Map

| File:line | What's there | When to look |
|---|---|---|
| `components/dispatcher/src/pipeline.rs:244` | `pipelined_ssd_to_gpu_zero_copy` — sliding-window baseline | **Template for the new P2P function**: copy this structure (VecDeque, prime window, pop completion, issue copy, submit next read) |
| `components/dispatcher/src/pipeline.rs:292` | VecDeque inflight tracking in zero-copy | **Exact pattern to replicate**: `inflight.push_back(i)` / `inflight.pop_front()` |
| `components/dispatcher/src/pipeline.rs:315` | Sliding-window main loop in zero-copy | Model for the new P2P main loop |
| `components/dispatcher/src/pipeline.rs:381` | Periodic sync every 16 completions | Model for P2P sync cadence (use `ring_size` instead of 16) |
| `components/dispatcher/src/pipeline.rs:200` | `PipelineRingP2P` struct (from iter-1 patch) | Reuse this struct and its constructor |
| `components/dispatcher/src/lib.rs:251` | `promote_and_serve` P2P branch | Wire the new sliding-window P2P call here |
| `components/dispatcher/src/lib.rs:686` | Pipeline ring allocation in initialize() | P2P ring allocation already wired here from iter-1 |
| `components/dispatcher/src/lib.rs:1091` | `queue_depth = 16 / num_queues` for batch_lookup | Wire P2P into batch_lookup cold path too |
| `components/gpu-services/src/dma.rs:353` | `create_spdk_dma_buffer_from_gpu_bar` | Core P2P buffer creation (proven working) |
| `components/gpu-services/src/dma.rs:477` | `create_p2p_ring_slot` (from iter-1) | P2P ring slot allocation (proven working) |
| `components/gpu-services/src/cuda_ffi.rs:66` | `CUDA_MEMCPY_DEVICE_TO_DEVICE = 3` | Constant for D2D copies |
| `components/gpu-services/src/cuda_ffi.rs:118` | `cudaMemcpyAsync(dst, src, count, kind, stream)` | FFI for async D2D copy |

## Code Targets

### h-main: Sliding-Window P2P (no DRAM backfill)

1. **`components/dispatcher/src/pipeline.rs`** — Rewrite `pipelined_ssd_to_gpu_p2p` function.
   - Location: Replace the existing batch-based implementation (if still present from iter-1, or add after line 401)
   - Structure: Mirror `pipelined_ssd_to_gpu_zero_copy` lines 244-401:
     - Create noop-free DmaBuffer wrappers for ring slots (for NVMe ReadAsync commands)
     - Collect dev_ptrs from ring slots
     - Use `VecDeque<usize>` to track in-flight ring slot indices
     - Prime window: submit `min(ring_size, num_chunks)` reads
     - Main loop: recv completion → pop slot index → issue `cudaMemcpyAsync(D2D)` → submit next read → push slot back
     - Sync every `ring_size` completions
     - Final sync both streams
   - Key difference from zero-copy: Use `cudaMemcpyAsync(dev_ptrs[slot_idx], ..., D2D, stream)` instead of `gpu.dma_copy_to_device_async(&guard, ...)`
   - Signature: `unsafe fn pipelined_ssd_to_gpu_p2p(drive, gpu, ring: &PipelineRingP2P, channels, mem_tier_ptr, gpu_dst, start_lba, total_bytes, max_queue_depth, backfill_dram: bool)`

2. **`components/dispatcher/src/lib.rs`** — Wire into promote_and_serve.
   - Line ~251: When P2P ring available AND `P2P_NO_BACKFILL` env set: skip `mt.insert`, call P2P with `backfill_dram=false`, register as BlockDevice-only entry
   - When P2P ring available AND env NOT set: allocate memory-tier slot, call P2P with `backfill_dram=true`, register memory-tier entry

3. **`components/gpu-services/src/dma.rs`** — Keep existing `P2pRingSlot`, `create_p2p_ring_slot`, `destroy_p2p_ring_slot` from iter-1 unchanged.

### h-ablation: With DRAM backfill

4. **`components/dispatcher/src/pipeline.rs`** — In the sliding-window loop, when `backfill_dram=true`: after issuing D2D copy, also issue `cudaMemcpyAsync(D2H)` from `dev_ptrs[slot_idx]` to `mem_tier_ptr+offset` on the same stream.

## What I Tried That Didn't Work

- **Iter-1 batch-based P2P:** 19% slower than baseline. The batch structure (submit all → wait all → copy all → sync) loses pipeline overlap. NEVER use this approach.
- **Direct DMA to final GPU destination (no staging):** NOT POSSIBLE because `gdr_pin_buffer` requires `cudaMalloc`-allocated memory; IPC-opened handles cannot be pinned.
- **Task description CLI flags `--metadata-pci`/`--data-pci`:** DO NOT EXIST. Use `--drive-count 1`.
- **`create_spdk_dma_buffer_from_gpu_bar(ptr, size, container_fd)`:** WRONG signature. Actual: `create_spdk_dma_buffer_from_gpu_bar(dev_ptr, size)` — only 2 parameters.

## What I Excluded and Why

- **Multi-drive scaling:** Uses 1 drive to isolate the P2P mechanism. Multi-drive introduces NVMe queue pair contention and NUMA effects. Test scaling in a future iteration if single-drive P2P succeeds.
- **Larger ring sizes (16, 32):** 8 slots is already sufficient since D2D completes ~200x faster than NVMe reads. Larger rings add memory overhead without pipeline benefit. Can revisit if 8 slots causes stalls.
- **Separate stream for D2H backfill:** Could improve h-ablation by parallelizing D2D and D2H on different streams. Excluded for simplicity — testing whether the simpler same-stream approach already works.
- **Cross-process P2P (`create_spdk_dma_buffer_from_phys`):** For Python client-provided GPU physical addresses. Our implementation keeps P2P internal to the server process.
- **`rte_extmem`/VFIO path (`create_spdk_dma_buffer_from_bar_direct`):** Alternative DPDK API path. GDRCopy + spdk_mem_register is simpler and proven working from iter-1.

## Evolution of Thinking

1. Initially re-examined iter-1's batch approach and confirmed the structural issue (RP-2).
2. Realized the zero-copy baseline's sliding-window is the correct template — specifically the VecDeque-based inflight tracking and per-completion processing.
3. Analyzed the D2D copy latency: at ~1.5 TB/s internal GPU bandwidth, 128 KiB copies take ~0.085µs. This means ring slot reuse is NEVER the bottleneck (NVMe reads take ~22µs per slot). The iter-1 concern about "ring slot reuse races" was overblown — it led to the conservative batch approach that caused the regression.
4. Identified that the real potential win is PCIe bandwidth: baseline uses PCIe in both directions (NVMe→DRAM upstream, DRAM→GPU downstream). P2P uses PCIe in one direction only (NVMe→GPU BAR1 via posted writes). At single-drive throughput (~5.9 GB/s) vs PCIe x16 capacity (~25 GB/s), contention may be minimal — but it should still yield measurable p99 latency improvement.
5. The h-main arm skips memory-tier entirely (no DRAM allocation, no backfill) for maximum P2P performance. The h-ablation adds DRAM back to measure RP-3 under proper conditions.

## Current Status

- **Validated:** Build compiles (`cargo build -p certus-server --release --features p2p` exits 0). P2P ring allocation primitives work (iter-1 proved data integrity). Sliding-window pattern works for baseline (observed 2.4 GB/s). `cudaMemcpyAsync` with `CUDA_MEMCPY_DEVICE_TO_DEVICE` constant exists in cuda_ffi.
- **Uncertain:** Whether NVMe DMA to GPU BAR1 achieves the same throughput as NVMe DMA to DRAM. Both traverse PCIe Gen4, but BAR1 writes may have different TLB/coherence behavior. If BAR1 DMA is slower, P2P will underperform even with perfect pipelining.
- **Suggested next:** If sliding-window P2P matches or beats baseline: (1) Test with larger block sizes (16 MiB, 64 MiB) where PCIe contention matters more. (2) Test multi-drive (2-4 drives) to approach PCIe x16 bandwidth ceiling. (3) Test with multiple clients to see if P2P's reduced PCIe contention gives better scaling. If P2P is still slower: investigate NVMe→BAR1 DMA throughput in isolation using the p2p_server benchmark binary.

## Warnings & Constraints

1. **Server must be killed and restarted between conditions** — the P2P ring is allocated once at init.
2. **Wait 4+ seconds after server start** — SPDK init, NVMe probe, extent format all take time. Port 50051 won't be ready immediately.
3. **GDRCopy requires gdrdrv kernel module** — verify with `ls /dev/gdrdrv`.
4. **Ring slot DmaBuffer wrappers must use `noop_free`** — the actual cleanup is handled by `destroy_p2p_ring_slot`. If you use a real free function, you'll double-free the BAR1 mapping.
5. **`std::mem::forget` the noop-free wrappers** — same pattern as `pipelined_ssd_to_gpu_zero_copy` line 396-398.
6. **D2D copy source is `dev_ptr`, NOT the DmaBuffer pointer** — The DmaBuffer's pointer is the BAR1 VA (for NVMe targeting). The `dev_ptr` (from cudaMalloc) is the device pointer for CUDA operations. Same physical memory, different VA.
7. **`cudaMemcpyAsync` stream parameter type** — The IGpuServices `GpuStream` wraps a pointer. Cast as `stream.0 as cuda_ffi::CudaStream` (see iter-1 patch line 392).
8. **The `P2P_NO_BACKFILL` env var selects h-main behavior** — set it for the no-backfill condition. Absence selects h-ablation behavior (with backfill).
9. **For h-main (no backfill), skip `mt.insert` entirely** — don't allocate DRAM that won't be used. Register entry as BlockDevice-only in dispatch-map so future lookups re-read from SSD.
