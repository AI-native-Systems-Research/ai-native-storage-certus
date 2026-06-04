# Problem Framing: P2P GPUDirect Storage for Cold Lookup Path

## Research Question

Can bypassing host DRAM in the cold lookup path — by issuing NVMe reads directly into GPU BAR1 memory via GDRCopy — improve throughput and reduce latency compared to the current NVMe→DRAM→GPU (cudaMemcpy H2D) pipeline?

The current cold path is implemented in `components/dispatcher/src/pipeline.rs:244` (`pipelined_ssd_to_gpu_zero_copy`). It reads NVMe data into CUDA-pinned DRAM (memory-tier pool) then copies from DRAM to GPU via `dma_copy_to_device_async` (cudaMemcpyAsync H2D). The P2P path eliminates the H2D copy by targeting NVMe DMA at GPU BAR1 physical addresses.

The GPU BAR1 DMA buffer creation function already exists at `components/gpu-services/src/dma.rs:353` (`create_spdk_dma_buffer_from_gpu_bar`). It uses GDRCopy to pin GPU memory, map it through BAR1, and register with SPDK for DMA.

## System Interface

- **Build command:** `cargo build -p certus-server --release --features p2p`
- **Server start:** `./target/release/certus-server --drive-count 1 --format`
- **Benchmark:** `python3 apps/python/certus-api-bench.py --clients 1 --num-objects 16 --iterations 10 --block-size 4194304`
- **Integrity check:** `python3 apps/python/certus-api-bench.py --clients 1 --verify-integrity --integrity-objects 8`

### Code Evidence

| Flag / mechanism | File:line | What it does |
|---|---|---|
| `--features p2p` | `apps/certus-server/Cargo.toml:9` | Enables `gpu-services/p2p` and `dispatcher/p2p` features |
| `--drive-count N` | `apps/certus-server/src/main.rs:34` | Auto-select N NVMe drives |
| `--format` | `apps/certus-server/src/main.rs:49` | Format extent managers on init |
| `pipelined_ssd_to_gpu_zero_copy` | `components/dispatcher/src/pipeline.rs:244` | Current cold path: NVMe→DRAM→GPU |
| `create_spdk_dma_buffer_from_gpu_bar` | `components/gpu-services/src/dma.rs:353` | P2P BAR1 buffer creation via GDRCopy |
| `gdrcopy_ffi` module | `components/gpu-services/src/gdrcopy_ffi.rs:1` | GDRCopy FFI bindings (gdr_open/pin/map) |
| `GPU_PAGE_SIZE` | `components/gpu-services/src/gdrcopy_ffi.rs:17` | 64 KiB GPU page alignment |
| `promote_and_serve` | `components/dispatcher/src/lib.rs:194` | Cold lookup entry point calling pipeline |
| `batch_lookup` cold path | `components/dispatcher/src/lib.rs:1045` | Parallel cold promotion with per-drive threads |

### Output Format

The benchmark prints results to stdout. Key metric lines:
```
  Lookup (cold)        avg=XXXXX.X us  p50=XXXXX.X us  p99=XXXXX.X us  min=XXXXX.X us  max=XXXXX.X us
                       per-client=X.XX GB/s  aggregate=X.XX GB/s
```

Parse: look for "Lookup (cold)" then "per-client=X.XX GB/s" on the next line.

## Baseline Command

```bash
# Build
cargo build -p certus-server --release --features p2p

# Start server (background, wait for port 50051)
./target/release/certus-server --drive-count 1 --format &
sleep 4

# Run benchmark
python3 apps/python/certus-api-bench.py --clients 1 --num-objects 16 --iterations 10 --block-size 4194304

# Kill server
kill %1
```

## Baseline Validation

The system description states baseline cold lookup throughput is 2.4 GB/s with a single drive. The benchmark reports "per-client=X.XX GB/s" after the "Lookup (cold)" section. The drive ceiling is 5.9 GB/s sequential read. The current bottleneck is the host-bounce cudaMemcpy H2D step which serializes against NVMe DMA completion.

## Experimental Conditions

### Condition 1: Baseline (NVMe→DRAM→GPU via cudaMemcpy)

No code changes. The existing `pipelined_ssd_to_gpu_zero_copy` function at `pipeline.rs:244` reads into DRAM then copies to GPU.

```bash
cargo build -p certus-server --release --features p2p
./target/release/certus-server --drive-count 1 --format &
sleep 4
python3 apps/python/certus-api-bench.py --clients 1 --num-objects 16 --iterations 10 --block-size 4194304
kill %1
```

### Condition 2: P2P (NVMe→GPU BAR1 direct)

Implement a new pipeline function `pipelined_ssd_to_gpu_p2p` in `pipeline.rs` that:
1. Allocates a ring of GPU BAR1 DMA buffers via `create_spdk_dma_buffer_from_gpu_bar` (one per ring slot, backed by cudaMalloc'd GPU memory)
2. Issues NVMe reads targeting the BAR1 buffers (DMA goes directly to GPU VRAM)
3. After each NVMe read completes, the data is already in GPU memory — issue a device-to-device memcpy from the BAR1 staging buffer to the final destination (`gpu_dst`)
4. No host DRAM intermediary, no cudaMemcpy H2D

Then modify `promote_and_serve` (lib.rs:194) and the `batch_lookup` cold path (lib.rs:1045) to call the P2P pipeline when the `p2p` feature is enabled.

The memory-tier DRAM copy is still needed for cache coherence (the memory-tier slot must be populated for future hot lookups), so the P2P path should also copy from GPU back to DRAM asynchronously after the GPU data is available.

```bash
cargo build -p certus-server --release --features p2p
./target/release/certus-server --drive-count 1 --format &
sleep 4
python3 apps/python/certus-api-bench.py --clients 1 --num-objects 16 --iterations 10 --block-size 4194304
kill %1
```

### Condition 3: P2P without memory-tier backfill (ablation)

Same as Condition 2 but skip the GPU→DRAM backfill copy. This tests whether the backfill overhead is significant. The trade-off is that subsequent lookups of the same key will always be cold (no warm-path optimization).

## Success Criteria

- **Primary:** P2P cold lookup throughput > baseline cold lookup throughput (directional improvement expected due to eliminating host-bounce copy)
- **Target:** Approach 5.9 GB/s drive ceiling (current: ~2.4 GB/s)
- **Hard constraints:** Build must succeed; data integrity check must PASS
- **Latency:** P2P p99 latency < baseline p99 latency

## Constraints

- Build must compile with `cargo build -p certus-server --release --features p2p`
- Data integrity must pass: `--verify-integrity --integrity-objects 8`
- Hardware requirements: gdrdrv kernel module, nvidia-peermem module, 2048 hugepages
- GPU BAR1 size limits the maximum concurrent P2P buffer pool (A30 has 32 GiB BAR1)
- GPU page alignment: allocations must be 64 KiB aligned (GPU_PAGE_SIZE)

## Prior Knowledge

This is the first iteration. No active principles exist yet.
