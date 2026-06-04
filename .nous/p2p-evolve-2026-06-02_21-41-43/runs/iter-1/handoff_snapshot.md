# Handoff: P2P GPUDirect Storage — Iteration 1

## Goal

Implement and benchmark a P2P (NVMe→GPU BAR1 direct DMA) cold lookup pipeline that bypasses host DRAM, comparing throughput against the current NVMe→DRAM→GPU path. The expected improvement eliminates the cudaMemcpy H2D step.

## Key Discoveries

1. **The P2P DMA buffer function already exists** — `gpu-services/src/dma.rs:353` (`create_spdk_dma_buffer_from_gpu_bar`) implements the full GDRCopy pin → BAR1 map → SPDK register sequence. It takes a `dev_ptr` (from cudaMalloc) and returns a `DmaBuffer` whose pointer is the BAR1 VA. NVMe DMA to this buffer lands in GPU VRAM.

2. **Current cold path uses `pipelined_ssd_to_gpu_zero_copy`** (pipeline.rs:244) — reads NVMe into memory-tier DRAM, then `dma_copy_to_device_async` (H2D) to GPU. The pipeline uses a sliding window of max_queue_depth=16 NVMe commands, 2 CUDA streams, and periodically syncs every 16 completions.

3. **The dispatcher feature chain is**: `certus-server/p2p` → `dispatcher/p2p` → `gpu-services/p2p`. The dispatcher's `p2p` feature currently does nothing dispatcher-side (just enables gpu-services/p2p). New code should be `#[cfg(feature = "p2p")]` gated.

4. **GPU BAR1 alignment requirement**: 64 KiB (`GPU_PAGE_SIZE` in gdrcopy_ffi.rs:17). All allocations must be aligned up to this boundary. The SPDK `max_transfer_size` is typically 128 KiB (chunk_size used in the pipeline), which is already a multiple of 64 KiB.

5. **Memory-tier cache coherence**: The cold path not only serves data to GPU but also populates the memory-tier DRAM slot so subsequent lookups are warm (H2D only). With P2P, we need a GPU→DRAM backfill to maintain this property. The ablation arm tests whether this backfill matters.

6. **GDRCopy setup is expensive** — gdr_open/pin/map per buffer takes milliseconds. Pre-allocating a ring (like existing `PipelineRing`) is essential. The ring should be allocated once at dispatcher init.

7. **Server CLI uses `--drive-count N` not `--metadata-pci`/`--data-pci`** — the main.rs shows `--device-pci` (multi-valued) or `--drive-count` (auto-discover). The earlier task description's flags are outdated. The benchmark uses default port 50051.

## System Interface

- **Build:** `cargo build -p certus-server --release --features p2p`
- **Run baseline:** `./target/release/certus-server --drive-count 1 --format` (then wait ~4s for port 50051)
- **Benchmark:** `python3 apps/python/certus-api-bench.py --clients 1 --num-objects 16 --iterations 10 --block-size 4194304`
- **Integrity:** `python3 apps/python/certus-api-bench.py --clients 1 --verify-integrity --integrity-objects 8`
- **Output format:** Stdout, look for `Lookup (cold)` section then `per-client=X.XX GB/s` on the following line.
- **Baseline result:** ~2.4 GB/s per-client cold lookup throughput (reported in task description).

## Code Map

| File:line | What's there | When to look |
|---|---|---|
| `components/dispatcher/src/pipeline.rs:244` | `pipelined_ssd_to_gpu_zero_copy` — current cold path | Adding the P2P version alongside it |
| `components/dispatcher/src/pipeline.rs:27` | `PipelineRing` struct (buffers + streams) | Model for the new P2P ring struct |
| `components/dispatcher/src/pipeline.rs:34` | `PipelineRing::new` — allocates CUDA-pinned SPDK buffers | Model for P2P ring allocation |
| `components/dispatcher/src/lib.rs:194` | `promote_and_serve` — single cold lookup entry | Wire P2P pipeline call here |
| `components/dispatcher/src/lib.rs:1045` | `batch_lookup` cold entries section | Wire P2P pipeline call here (parallel) |
| `components/dispatcher/src/lib.rs:686` | Pipeline ring allocation in initialize() | Allocate P2P ring here too |
| `components/gpu-services/src/dma.rs:353` | `create_spdk_dma_buffer_from_gpu_bar` | Core P2P buffer creation |
| `components/gpu-services/src/dma.rs:292` | `GdrMappingState` + cleanup tracking | Understand cleanup lifecycle |
| `components/gpu-services/src/gdrcopy_ffi.rs:1` | GDRCopy FFI types and functions | Reference for pin/map calls |
| `components/gpu-services/src/lib.rs:31` | `pub mod gdrcopy_ffi` (gated on `feature = "p2p"`) | Module visibility |
| `apps/certus-server/Cargo.toml:9` | `p2p = ["gpu-services/p2p", "dispatcher/p2p"]` | Feature propagation chain |
| `apps/certus-server/src/main.rs:109` | `initialize_component_stack` | Understand full init sequence |

## Code Targets

### h-main: P2P Pipeline

1. **`components/dispatcher/src/pipeline.rs`** — Add `PipelineRingP2P` struct and `pipelined_ssd_to_gpu_p2p` function.
   - Location: After line 401 (end of existing `pipelined_ssd_to_gpu_zero_copy`)
   - Why here: Same module, follows the existing pipeline pattern
   - The new struct needs: Vec of DmaBuffers (BAR1 VAs for NVMe targeting), Vec of device pointers (for D2D memcpy), and CUDA streams
   - The function issues ReadAsync with BAR1 buffers, then cudaMemcpyDeviceToDevice to final dst

2. **`components/gpu-services/src/dma.rs`** — Add `create_p2p_ring_buffer` helper.
   - Location: After line 466 (end of `create_spdk_dma_buffer_from_gpu_bar`)
   - Why: Encapsulates cudaMalloc + create_spdk_dma_buffer_from_gpu_bar into one call that returns both the DmaBuffer and the device pointer

3. **`components/dispatcher/src/lib.rs`** — Wire P2P into cold path.
   - Lines 686-694: Add P2P ring allocation (feature-gated)
   - Lines 194-288 (`promote_and_serve`): Call P2P function when available
   - Lines 1160-1173 (batch_lookup inner loop): Call P2P function when available

### h-ablation: Skip DRAM backfill

4. **`components/dispatcher/src/pipeline.rs`** — Add `backfill_dram: bool` parameter to P2P function.
5. **`components/dispatcher/src/lib.rs`** — Read `P2P_NO_BACKFILL` env var; when set, skip memory-tier insert and backfill.

## What I Tried That Didn't Work

- The task description mentions `--metadata-pci` and `--data-pci` flags — these DO NOT EXIST in certus-server's CLI. The actual flags are `--device-pci` (multi-valued) or `--drive-count`. Use `--drive-count 1`.
- The task description says `create_spdk_dma_buffer_from_gpu_bar(gpu_ptr, size, container_fd)` takes a container_fd — this is WRONG. The actual signature is `create_spdk_dma_buffer_from_gpu_bar(dev_ptr: *mut c_void, size: usize)` with only 2 parameters.

## What I Excluded and Why

- **Multi-drive P2P**: This iteration uses 1 drive to isolate the P2P mechanism. Multi-drive introduces NVMe queue pair contention and NUMA effects that muddy the signal. Next iteration can test scaling.
- **BAR1 size limits**: The A30 has 32 GiB BAR1, far more than the ring buffer needs (8 * 128KiB = 1 MiB). Not a constraint for this experiment.
- **Cross-process P2P** (`create_spdk_dma_buffer_from_phys` at dma.rs:547): This is for the case where the Python client provides a GPU physical address. Our implementation keeps P2P internal to the server process, so we use the simpler `create_spdk_dma_buffer_from_gpu_bar` path.
- **rte_extmem/VFIO path** (`create_spdk_dma_buffer_from_bar_direct` at dma.rs:635): This alternative uses DPDK APIs directly. The GDRCopy + spdk_mem_register path is simpler and already tested. Can revisit if the spdk_mem_register path fails.

## Evolution of Thinking

Initially assumed the task was simple flag-flipping, but discovered:
1. No P2P pipeline function exists yet — only the DMA buffer allocation primitive is ready
2. The cold path must maintain memory-tier coherence (backfill DRAM), which adds complexity
3. The PipelineRing pre-allocation pattern is essential — can't allocate GDRCopy mappings per call
4. The device-to-device memcpy (BAR1 staging → final destination) is needed because NVMe reads target fixed ring slots, not the final destination directly (alignment and reuse constraints)

## Current Status

- **Validated:** Feature propagation chain, DMA buffer API signatures, pipeline structure, benchmark output format, server CLI flags
- **Uncertain:** Whether spdk_mem_register works correctly with GDRCopy BAR1 mappings in this SPDK version (the code exists but hasn't been tested via this specific path). If it fails, fall back to `create_spdk_dma_buffer_from_bar_direct` (DPDK rte_extmem path).
- **Suggested next:** If P2P works, test with max_queue_depth > 16 to saturate the NVMe→GPU PCIe path. Also test multi-drive (2-4 drives) to see if aggregate throughput approaches PCIe x16 bandwidth (~25 GB/s Gen4).

## Warnings & Constraints

1. **Server must be killed and restarted between conditions** — the pipeline ring is allocated once at init; changing between baseline and P2P requires a server restart.
2. **Wait 4+ seconds after server start** — SPDK init, NVMe probe, extent format all take time. Port 50051 won't be ready immediately.
3. **GDRCopy requires root-equivalent access** — `gdrdrv` device access. The server process needs appropriate permissions.
4. **Integrity check is essential** — P2P DMA targeting GPU memory can silently corrupt if IOMMU mapping is wrong. Always run `--verify-integrity` after code changes.
5. **cudaMemcpyDeviceToDevice for staging→final** — Don't try to DMA directly to `gpu_dst` because: (a) gpu_dst comes from cudaIpcOpenMemHandle and may not be page-aligned to GPU_PAGE_SIZE, (b) ring buffer reuse requires fixed-size aligned slots.
6. **The `dma_copy_to_device_async` function in IGpuServices takes a DmaBuffer src** — for P2P, you need a new interface method or direct CUDA calls for D2D copy since the source is GPU memory, not a DmaBuffer.
